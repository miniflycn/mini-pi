<script setup lang="ts">
import { readUIMessageStream } from 'ai'
import type { ChatStatus, UIMessage } from 'ai'

const route = useRoute()
const toast = useToast()
const config = usePiRemoteConfig()
const remote = usePiRemote()
const { model } = useModels()

const threadId = computed(() => String(route.params.id))

const messages = ref<UIMessage[]>([])
const status = ref<ChatStatus>('ready')
const chatError = ref<Error | undefined>(undefined)
const input = ref('')
const title = ref<string | null>(null)
const loadingThread = ref(true)
const { level: thinkingLevel, setLevel: setThinkingLevel } = useThinkingLevel()

let activeAbortController: AbortController | null = null
const isStopping = ref(false)

async function loadThread() {
  loadingThread.value = true
  chatError.value = undefined
  try {
    // The remote controller only keeps messages in an active session, so we
    // must reopen the thread before reading or streaming it.
    await remote.openThread(threadId.value)
    const loaded = await remote.getMessages(threadId.value)
    messages.value = loaded.map(piMessageToUIMessage)
    status.value = 'ready'
  } catch (err) {
    chatError.value = err instanceof Error ? err : new Error(String(err))
    status.value = 'error'
  } finally {
    loadingThread.value = false
  }
}

onMounted(async () => {
  if (!config.isConfigured.value) {
    return
  }
  await loadThread()
  if (thinkingLevel.value) {
    try {
      await setThinkingLevel(threadId.value, thinkingLevel.value)
    } catch (err) {
      toast.add({
        description: err instanceof Error ? err.message : String(err),
        icon: 'i-lucide-alert-circle',
        color: 'error'
      })
    }
  }
  const pendingPrompt = useState<string | null>(`pending-prompt-${threadId.value}`, () => null)
  if (pendingPrompt.value) {
    const text = pendingPrompt.value
    pendingPrompt.value = null
    input.value = text
    await handleSubmit()
  }
})

onUnmounted(() => {
  activeAbortController?.abort()
})

watch(threadId, async () => {
  if (!config.isConfigured.value) return
  await loadThread()
})

async function handleSubmit() {
  const text = input.value.trim()
  if (!text || status.value === 'streaming' || status.value === 'submitted') return

  // Optimistically add the user message.
  const userMessage: UIMessage = {
    id: crypto.randomUUID(),
    role: 'user',
    parts: [{ type: 'text', text, state: 'done' }]
  }
  messages.value.push(userMessage)
  input.value = ''

  const abortController = new AbortController()
  activeAbortController = abortController
  isStopping.value = false

  try {
    status.value = 'submitted'
    chatError.value = undefined
    const stream = await remote.sendMessageStream(threadId.value, text, abortController.signal)
    for await (const assistantMessage of readUIMessageStream({ stream })) {
      if (isStopping.value) {
        break
      }
      const index = messages.value.findIndex(message => message.id === assistantMessage.id)
      if (index >= 0) {
        messages.value[index] = assistantMessage
      } else {
        messages.value.push(assistantMessage)
      }
      status.value = 'streaming'
    }
    status.value = 'ready'
  } catch (err) {
    if (abortController.signal.aborted || isStopping.value) {
      status.value = 'ready'
      return
    }
    status.value = 'error'
    chatError.value = err instanceof Error ? err : new Error(String(err))
    toast.add({
      description: chatError.value.message,
      icon: 'i-lucide-alert-circle',
      color: 'error'
    })
  } finally {
    isStopping.value = false
    if (activeAbortController === abortController) {
      activeAbortController = null
    }
  }
}

async function handleAbort() {
  if (status.value !== 'streaming' && status.value !== 'submitted') return

  isStopping.value = true
  activeAbortController?.abort()
  status.value = 'ready'

  try {
    await remote.abortThread(threadId.value)
  } catch (err) {
    toast.add({
      description: err instanceof Error ? err.message : String(err),
      icon: 'i-lucide-alert-circle',
      color: 'error'
    })
  }
}

watch(model, async (newModel) => {
  if (!threadId.value) return
  try {
    await remote.setModel(threadId.value, newModel)
  } catch (err) {
    toast.add({
      description: err instanceof Error ? err.message : String(err),
      icon: 'i-lucide-alert-circle',
      color: 'error'
    })
  }
})

watch(thinkingLevel, async (newLevel) => {
  if (!threadId.value || !newLevel || loadingThread.value) return
  try {
    await setThinkingLevel(threadId.value, newLevel)
  } catch (err) {
    toast.add({
      description: err instanceof Error ? err.message : String(err),
      icon: 'i-lucide-alert-circle',
      color: 'error'
    })
  }
})
</script>

<template>
  <UDashboardPanel
    id="chat"
    class="relative min-h-0"
    :ui="{ body: 'p-0 sm:p-0 overscroll-none' }"
  >
    <template #header>
      <Navbar>
        <template #title>
          <ChatTitle
            :chat-id="String(threadId)"
            :title="title"
            :is-owner="true"
            @update:title="title = $event"
          />
        </template>
      </Navbar>
    </template>

    <template #body>
      <UContainer class="flex-1 flex flex-col gap-4 sm:gap-6">
        <template v-if="loadingThread">
          <div class="flex-1 flex items-center justify-center">
            <ChatLoader />
          </div>
        </template>

        <template v-else>
          <UChatMessages
            should-auto-scroll
            :messages="messages"
            :status="status"
            :spacing-offset="160"
            class="pt-(--ui-header-height) pb-4 sm:pb-6"
          >
            <template #indicator>
              <div class="flex items-center gap-1.5">
                <ChatIndicator />
                <UChatShimmer text="Thinking..." class="text-sm" />
              </div>
            </template>

            <template #content="{ message }">
              <ChatMessageContent :message="message" :editing="false" />
            </template>
          </UChatMessages>
          <ChatBox
            v-if="config.isConfigured.value"
            v-model="input"
            :status="status"
            :error="chatError"
            class="sticky bottom-0 [view-transition-name:chat-prompt] rounded-b-none z-10 pb-safe"
            @submit="handleSubmit"
            @stop="handleAbort"
          />
        </template>
      </UContainer>
    </template>
  </UDashboardPanel>
</template>

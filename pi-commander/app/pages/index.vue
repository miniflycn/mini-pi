<script setup lang="ts">
const input = ref('')
const creating = ref(false)
const selectedWorkspaceId = ref<string | null>(null)

let createAbortController: AbortController | null = null

const config = usePiRemoteConfig()
const remote = usePiRemote()
const init = usePiInit()
const { model } = useModels()
const { level: thinkingLevel } = useThinkingLevel()
const settings = usePiRemoteSettingsModal()
const toast = useToast()

const workspaces = init.workspaces
const loadingWorkspaces = computed(() => init.loading.value && init.workspaces.value.length === 0)
const workspacesError = init.error

// Auto-select the first workspace when the list loads.
watchEffect(() => {
  const list = workspaces.value
  if (list.length > 0 && !selectedWorkspaceId.value) {
    selectedWorkspaceId.value = list[0]!.id
  }
})

// Clear selection when the list empties (e.g. disconnecting).
watchEffect(() => {
  if (workspaces.value.length === 0) {
    selectedWorkspaceId.value = null
  }
})

async function openSettings() {
  await settings.open()
}

async function createChat(prompt: string) {
  if (!config.isConfigured.value) {
    await openSettings()
    if (!config.isConfigured.value) return
  }

  if (!selectedWorkspaceId.value) {
    toast.add({
      description: 'Please select a workspace first',
      icon: 'i-lucide-alert-circle',
      color: 'error'
    })
    return
  }

  input.value = prompt
  creating.value = true
  createAbortController = new AbortController()

  try {
    const { thread_id } = await remote.createThread(
      model.value || undefined,
      selectedWorkspaceId.value,
      createAbortController.signal
    )
    if (createAbortController.signal.aborted) {
      return
    }
    if (thinkingLevel.value) {
      await remote.setThinkingLevel(thread_id, thinkingLevel.value, createAbortController.signal)
    }
    if (createAbortController.signal.aborted) {
      return
    }
    const pendingPrompt = useState<string | null>(`pending-prompt-${thread_id}`, () => null)
    pendingPrompt.value = prompt
    await navigateTo(`/chat/${thread_id}`)
  } catch (err) {
    if (createAbortController?.signal.aborted) {
      return
    }
    toast.add({
      description: err instanceof Error ? err.message : String(err),
      icon: 'i-lucide-alert-circle',
      color: 'error'
    })
  } finally {
    creating.value = false
    createAbortController = null
  }
}

async function onSubmit() {
  await createChat(input.value)
}

function handleStopCreate() {
  creating.value = false
  createAbortController?.abort()
}
</script>

<template>
  <UDashboardPanel
    id="home"
    class="min-h-0"
    :ui="{ body: 'p-0 sm:p-0' }"
  >
    <template #header>
      <Navbar />
    </template>

    <template #body>
      <UContainer class="flex-1 flex flex-col justify-center gap-4 sm:gap-6 py-8">
        <div class="flex flex-col gap-2">
          <h1 class="text-3xl sm:text-4xl text-highlighted font-bold">
            pi commander
          </h1>
          <p class="text-muted">
            Chat with your mini-pi agent through its Cloudflare tunnel.
          </p>
        </div>

        <div class="flex flex-col gap-3">
          <div class="flex items-center justify-between">
            <h2 class="text-sm font-semibold text-highlighted">
              Workspace
            </h2>
            <UButton
              v-if="!config.isConfigured.value"
              size="xs"
              color="neutral"
              variant="ghost"
              label="Configure"
              @click="openSettings"
            />
          </div>

          <div
            v-if="loadingWorkspaces"
            class="grid grid-cols-1 sm:grid-cols-2 gap-3"
          >
            <USkeleton class="h-20" />
            <USkeleton class="h-20" />
          </div>

          <div
            v-else-if="workspacesError"
            class="text-sm text-error"
          >
            {{ workspacesError.message }}
          </div>

          <div
            v-else-if="!workspaces.length"
            class="text-sm text-muted"
          >
            No workspaces found. Create one in mini-pi first.
          </div>

          <div
            v-else
            class="grid grid-cols-1 sm:grid-cols-2 gap-3"
          >
            <button
              v-for="workspace in workspaces"
              :key="workspace.id"
              type="button"
              class="text-left p-4 rounded-lg border transition-colors"
              :class="selectedWorkspaceId === workspace.id
                ? 'border-primary bg-primary/5'
                : 'border-default bg-default hover:bg-muted/50'"
              @click="selectedWorkspaceId = workspace.id"
            >
              <div class="flex items-start gap-3">
                <UIcon
                  name="i-lucide-folder"
                  class="size-5 mt-0.5 text-muted"
                />
                <div class="min-w-0 flex-1">
                  <div class="font-medium text-highlighted truncate">
                    {{ workspace.name }}
                  </div>
                  <div class="text-xs text-muted truncate">
                    {{ workspace.path }}
                  </div>
                </div>
                <UIcon
                  v-if="selectedWorkspaceId === workspace.id"
                  name="i-lucide-check"
                  class="size-5 text-primary"
                />
              </div>
            </button>
          </div>
        </div>

        <ChatBox
          v-model="input"
          :status="creating ? 'streaming' : 'ready'"
          :disabled="!selectedWorkspaceId || loadingWorkspaces"
          :submit-disabled="creating || !selectedWorkspaceId"
          class="[view-transition-name:chat-prompt] pb-safe"
          @submit="onSubmit"
          @stop="handleStopCreate"
        />

        <p
          v-if="!selectedWorkspaceId && !loadingWorkspaces && workspaces.length"
          class="text-xs text-muted"
        >
          Select a workspace above to start chatting.
        </p>
      </UContainer>
    </template>
  </UDashboardPanel>
</template>

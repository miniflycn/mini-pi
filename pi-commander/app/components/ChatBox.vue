<script setup lang="ts">
import type { ChatStatus } from 'ai'

const props = withDefaults(defineProps<{
  status?: ChatStatus
  error?: Error
  disabled?: boolean
  submitDisabled?: boolean
}>(), {
  status: 'ready'
})

const model = defineModel<string>({ default: '' })
const emit = defineEmits<{
  submit: []
  stop: []
}>()

const toast = useToast()
const voice = useVoiceInput()

const canUseVoice = computed(() =>
  voice.supported
  && props.status !== 'streaming'
  && props.status !== 'submitted'
)

async function toggleVoiceInput() {
  if (voice.isRecording.value) {
    try {
      const blob = await voice.stopRecording()
      const text = await voice.transcribe(blob)
      model.value = model.value.trimEnd()
      model.value = model.value ? `${model.value} ${text}` : text
    } catch (err) {
      toast.add({
        description: err instanceof Error ? err.message : String(err),
        icon: 'i-lucide-alert-circle',
        color: 'error'
      })
    }
    return
  }

  try {
    await voice.startRecording()
  } catch (err) {
    toast.add({
      description: err instanceof Error ? err.message : String(err),
      icon: 'i-lucide-alert-circle',
      color: 'error'
    })
  }
}

function onSubmit() {
  emit('submit')
}
</script>

<template>
  <UChatPrompt
    v-model="model"
    :status="status"
    :error="error"
    :disabled="disabled || status === 'submitted'"
    variant="subtle"
    :ui="{ base: 'px-1.5' }"
    @submit="onSubmit"
  >
    <template #footer>
      <div class="flex items-center gap-1">
        <ModelSelect />
        <ThinkingLevelSelect />
      </div>

      <div class="flex items-center gap-1">
        <UButton
          :icon="voice.isRecording.value ? 'i-lucide-square' : 'i-lucide-mic'"
          :color="voice.isRecording.value ? 'error' : 'neutral'"
          :loading="voice.isTranscribing.value"
          :disabled="!voice.isRecording.value && !canUseVoice"
          variant="ghost"
          size="sm"
          :aria-label="voice.isRecording.value ? 'Stop recording' : 'Start voice input'"
          @click="toggleVoiceInput"
        />

        <UChatPromptSubmit
          :status="status"
          color="neutral"
          size="sm"
          type="button"
          :disabled="submitDisabled"
          @stop="emit('stop')"
        />
      </div>
    </template>
  </UChatPrompt>
</template>

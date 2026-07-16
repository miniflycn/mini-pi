<script setup lang="ts">
import type { DynamicToolUIPart } from 'ai'

const props = defineProps<{
  invocation: DynamicToolUIPart
}>()

interface SendFileDetails {
  path?: string
  mime_type?: string
  size?: number
  data?: string
}

const details = computed<SendFileDetails>(() => {
  const raw = (props.invocation as { details?: Record<string, unknown> }).details
  if (raw && typeof raw === 'object') {
    return {
      path: typeof raw.path === 'string' ? raw.path : undefined,
      mime_type: typeof raw.mime_type === 'string' ? raw.mime_type : undefined,
      size: typeof raw.size === 'number' ? raw.size : undefined,
      data: typeof raw.data === 'string' ? raw.data : undefined
    }
  }
  return {}
})

const parsedOutput = computed(() => {
  const output = typeof props.invocation.output === 'string'
    ? props.invocation.output
    : ''
  // Expected: "Sent file: <name> (<mime>, <size> bytes)"
  const prefix = 'Sent file: '
  if (!output.startsWith(prefix)) return null
  const rest = output.slice(prefix.length)
  const parenIdx = rest.lastIndexOf('(')
  if (parenIdx === -1) return null
  const name = rest.slice(0, parenIdx).trim()
  const inner = rest.slice(parenIdx + 1, rest.length - 1)
  const commaIdx = inner.lastIndexOf(',')
  if (commaIdx === -1) return null
  const mime = inner.slice(0, commaIdx).trim()
  const sizePart = inner.slice(commaIdx + 1).trim().split(/\s+/)[0] ?? '0'
  const size = Number.parseInt(sizePart, 10)
  return { name, mime, size: Number.isNaN(size) ? 0 : size }
})

const fileName = computed(() => {
  if (details.value.path) {
    const parts = details.value.path.split(/[/\\]/)
    return parts[parts.length - 1] || details.value.path
  }
  return parsedOutput.value?.name || 'Unknown file'
})

const mimeType = computed(() => {
  return details.value.mime_type || parsedOutput.value?.mime || 'application/octet-stream'
})

const size = computed(() => {
  return details.value.size ?? parsedOutput.value?.size ?? 0
})

const isLoading = computed(() => {
  return props.invocation.state === 'input-streaming' || props.invocation.state === 'input-available'
})

function formatSize(bytes: number): string {
  if (bytes === 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB']
  let value = bytes
  let unitIndex = 0
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024
    unitIndex++
  }
  return `${value.toFixed(unitIndex === 0 ? 0 : 1)} ${units[unitIndex]}`
}

function statusText(): string {
  if (isLoading.value) return 'Sending file...'
  if (props.invocation.state === 'output-error') return 'Failed to send file'
  return 'Sent file'
}

const hasInlineData = computed(() => Boolean(details.value.data))
const canDownload = computed(() => {
  return !isLoading.value && (hasInlineData.value || Boolean(details.value.path))
})

const isDownloading = ref(false)
const downloadError = ref<string | null>(null)

function base64ToBlob(base64: string, mime: string): Blob {
  const bin = atob(base64)
  const len = bin.length
  const bytes = new Uint8Array(len)
  for (let i = 0; i < len; i++) {
    bytes[i] = bin.charCodeAt(i)
  }
  return new Blob([bytes], { type: mime })
}

function triggerDownload(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = name
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  URL.revokeObjectURL(url)
}

async function onDownload() {
  if (!canDownload.value || isDownloading.value) return
  downloadError.value = null
  isDownloading.value = true

  try {
    if (details.value.data) {
      const blob = base64ToBlob(details.value.data, mimeType.value)
      triggerDownload(blob, fileName.value)
    } else if (details.value.path) {
      const remote = usePiRemote()
      const blob = await remote.downloadFile(details.value.path, mimeType.value)
      triggerDownload(blob, fileName.value)
    }
  } catch (err) {
    downloadError.value = err instanceof Error ? err.message : 'Download failed'
  } finally {
    isDownloading.value = false
  }
}
</script>

<template>
  <UChatTool
    :text="statusText()"
    icon="i-lucide-file"
    :loading="isLoading"
    variant="card"
    chevron="leading"
  >
    <div class="flex items-center gap-3 p-2 rounded-lg bg-muted/50">
      <UIcon name="i-lucide-file" class="size-8 text-muted shrink-0" />
      <div class="min-w-0 flex-1">
        <div class="text-sm font-medium truncate">
          {{ fileName }}
        </div>
        <div class="text-xs text-muted">
          {{ mimeType }} • {{ formatSize(size) }}
        </div>
        <div v-if="downloadError" class="text-xs text-red-500 mt-1">
          {{ downloadError }}
        </div>
      </div>
      <UButton
        v-if="canDownload"
        size="xs"
        color="neutral"
        variant="ghost"
        :loading="isDownloading"
        :icon="isDownloading ? undefined : 'i-lucide-download'"
        :aria-label="hasInlineData ? 'Download file' : 'Download file from remote'"
        @click="onDownload"
      />
    </div>
  </UChatTool>
</template>

<script setup lang="ts">
const QrScanner = defineAsyncComponent(() => import('~/components/QrScanner.vue'))

const props = defineProps<{
  blocking?: boolean
}>()

const emit = defineEmits<{ close: [boolean] }>()

const isBlocking = computed(() => props.blocking ?? false)

const config = usePiRemoteConfig()
const remote = usePiRemote()
const toast = useToast()

const baseUrl = ref(config.baseUrl.value)
const token = ref(config.token.value)
const testing = ref(false)
const scanning = ref(false)
const scanError = ref('')

async function checkConnection(): Promise<boolean> {
  testing.value = true
  try {
    config.set({ baseUrl: baseUrl.value, token: token.value })
    const status = await remote.status()
    toast.add({
      title: 'Connection successful',
      description: status.tunnel_url
        ? `Remote API is running at ${status.tunnel_url}`
        : 'Remote API is running',
      icon: 'i-lucide-check-circle',
      color: 'success'
    })
    return true
  } catch (err) {
    toast.add({
      title: 'Connection failed',
      description: err instanceof Error ? err.message : String(err),
      icon: 'i-lucide-alert-circle',
      color: 'error'
    })
    return false
  } finally {
    testing.value = false
  }
}

async function save() {
  const ok = await checkConnection()
  if (!ok) return
  emit('close', true)
}

function onScan(value: string) {
  scanning.value = false
  scanError.value = ''

  // The mini-pi QR code contains the tunnel URL.
  try {
    const url = new URL(value)
    baseUrl.value = `${url.protocol}//${url.host}`
    toast.add({
      title: 'QR code scanned',
      description: 'Tunnel URL has been filled in.',
      icon: 'i-lucide-check-circle',
      color: 'success'
    })
  } catch {
    baseUrl.value = value.trim()
    toast.add({
      title: 'QR code scanned',
      description: 'Scanned value has been filled in.',
      icon: 'i-lucide-check-circle',
      color: 'success'
    })
  }
}

function onScanError(message: string) {
  scanning.value = false
  scanError.value = message
}
</script>

<template>
  <UModal
    title="Remote API settings"
    :description="isBlocking
      ? 'Configure your mini-pi tunnel before using the app.'
      : 'Connect to your mini-pi instance through its Cloudflare tunnel.'
    "
    :dismissible="!isBlocking"
    :close="false"
    :ui="{
      footer: 'flex-row-reverse justify-start'
    }"
  >
    <template #body>
      <div class="flex flex-col gap-4">
        <UAlert
          v-if="isBlocking"
          color="warning"
          icon="i-lucide-alert-triangle"
          title="Configuration required"
          description="Enter your tunnel URL to continue. The bearer token is only required if your mini-pi instance has one configured."
        />

        <UAlert
          v-if="scanError"
          color="error"
          icon="i-lucide-alert-circle"
          title="Scan failed"
          :description="scanError"
        />

        <QrScanner
          v-if="scanning"
          @scan="onScan"
          @error="onScanError"
          @cancel="scanning = false"
        />

        <template v-else>
          <UFormField label="Tunnel URL">
            <UInput
              v-model="baseUrl"
              placeholder="https://abc123.trycloudflare.com"
              class="w-full"
              :ui="{ root: 'w-full' }"
            />
          </UFormField>

          <UFormField label="Bearer token" description="Optional unless your mini-pi instance requires authentication.">
            <UInput
              v-model="token"
              type="password"
              placeholder="your-secret-token"
              class="w-full"
              :ui="{ root: 'w-full' }"
            />
          </UFormField>

          <UButton
            color="neutral"
            variant="outline"
            icon="i-lucide-scan-line"
            label="Scan QR code"
            block
            @click="scanning = true"
          />
        </template>
      </div>
    </template>

    <template #footer>
      <UButton
        label="Save"
        :loading="testing"
        :disabled="!baseUrl.trim() || testing"
        @click="save"
      />
      <UButton
        v-if="!isBlocking"
        color="neutral"
        variant="ghost"
        label="Cancel"
        @click="emit('close', false)"
      />
    </template>
  </UModal>
</template>

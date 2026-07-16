<script setup lang="ts">
const colorMode = useColorMode()
const config = usePiRemoteConfig()
const settings = usePiRemoteSettingsModal()

const color = computed(() => colorMode.value === 'dark' ? '#1b1718' : 'white')

useHead({
  meta: [
    { key: 'theme-color', name: 'theme-color', content: color }
  ]
})

const title = 'pi commander'
const description = 'A web client for mini-pi remote control through Cloudflare Tunnel.'

useSeoMeta({
  title,
  description,
  ogTitle: title,
  ogDescription: description,
  twitterCard: 'summary_large_image'
})

watch(() => config.isConfigured.value, async (isConfigured) => {
  if (!isConfigured) {
    await settings.open(true)
  }
}, { immediate: true })
</script>

<template>
  <UApp :toaster="{ position: 'top-right' }" :tooltip="{ delayDuration: 200 }">
    <NuxtLoadingIndicator color="var(--ui-primary)" />

    <NuxtLayout>
      <NuxtPage />
    </NuxtLayout>
  </UApp>
</template>

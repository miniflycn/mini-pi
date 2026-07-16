<script setup lang="ts">
const config = usePiRemoteConfig()
const remote = usePiRemote()
const settings = usePiRemoteSettingsModal()
const toast = useToast()

const sidebarOpen = ref(false)
const searchOpen = ref(false)
const chats = ref<{
  id: string
  label: string
  to: string
  icon: string
  createdAt: string
}[]>([])
const loadingChats = ref(false)

async function refreshChats() {
  if (!config.isConfigured.value) {
    chats.value = []
    return
  }
  loadingChats.value = true
  try {
    const { threads } = await remote.listThreads()
    chats.value = threads.map(thread => ({
      id: String(thread.id),
      label: thread.title || 'Untitled',
      to: `/chat/${thread.id}`,
      icon: 'i-lucide-message-circle',
      createdAt: thread.updated_at || thread.created_at
    }))
  } catch (err) {
    chats.value = []
    if (isBackendUnavailableError(err)) {
      toast.add({
        title: 'Remote API unreachable',
        description: 'Could not connect to your mini-pi backend. Please check your tunnel URL.',
        icon: 'i-lucide-alert-circle',
        color: 'error'
      })
      await settings.open()
    }
  } finally {
    loadingChats.value = false
  }
}

onMounted(() => {
  refreshChats()
})

watch([() => config.baseUrl.value, () => config.token.value], () => {
  refreshChats()
})

const { groups } = useChats(chats)

const items = computed(() => groups.value?.flatMap((group) => {
  return [{
    label: group.label,
    type: 'label' as const
  }, ...group.items.map(item => ({
    ...item,
    slot: 'chat' as const,
    icon: undefined,
    class: item.label === 'Untitled' ? 'text-muted' : ''
  }))]
}))

defineShortcuts({
  meta_o: () => {
    navigateTo('/')
  }
})
</script>

<template>
  <UDashboardGroup unit="rem" class="pb-safe">
    <UDashboardSidebar
      id="default"
      v-model:open="sidebarOpen"
      :min-size="12"
      collapsible
      resizable
      :menu="{ inset: true }"
      class="border-r-0 py-4 dark:[--ui-bg-elevated:var(--ui-color-neutral-900)]"
    >
      <template #header="{ collapsed }">
        <NuxtLink v-if="!collapsed" to="/" class="flex items-end gap-0.5">
          <Logo class="h-8 w-auto shrink-0" />
          <span class="text-xl font-bold text-highlighted">Chat</span>
        </NuxtLink>

        <UDashboardSidebarCollapse class="ms-auto" />
      </template>

      <template #default="{ collapsed }">
        <UNavigationMenu
          :items="[{
            label: 'New chat',
            to: '/',
            kbds: ['meta', 'o'],
            icon: 'i-lucide-circle-plus'
          }, {
            label: 'Search',
            icon: 'i-lucide-search',
            kbds: ['meta', 'k'],
            onSelect: () => {
              searchOpen = true
            }
          }]"
          :collapsed="collapsed"
          orientation="vertical"
        >
          <template #item-trailing="{ item }">
            <div v-if="item.kbds?.length" class="flex items-center gap-px opacity-0 group-hover:opacity-100 transition-opacity">
              <UKbd
                v-for="kbd in item.kbds"
                :key="kbd"
                :value="kbd"
                size="sm"
                variant="soft"
                class="bg-accented/50"
              />
            </div>
          </template>
        </UNavigationMenu>

        <UNavigationMenu
          v-if="!collapsed"
          :items="items"
          :collapsed="collapsed"
          orientation="vertical"
        />
      </template>

      <template #footer="{ collapsed }">
        <UButton
          :label="collapsed ? undefined : 'Pi settings'"
          icon="i-lucide-globe"
          color="neutral"
          variant="ghost"
          block
          :square="collapsed"
          class="data-[state=open]:bg-elevated"
          @click="settings.open()"
        />

        <UserMenu :collapsed="collapsed" />
      </template>
    </UDashboardSidebar>

    <UDashboardSearch
      v-model:open="searchOpen"
      placeholder="Search chats..."
      :groups="[{
        id: 'links',
        items: [{
          label: 'New chat',
          to: '/',
          icon: 'i-lucide-circle-plus',
          kbds: ['meta', 'o']
        }]
      }, ...groups]"
    />

    <div class="flex-1 flex m-4 lg:ml-0 rounded-lg ring ring-default bg-default/75 shadow min-w-0 overflow-hidden">
      <slot />
    </div>
  </UDashboardGroup>
</template>

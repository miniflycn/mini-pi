<script setup lang="ts">
import { isReasoningUIPart, isTextUIPart, isToolUIPart, getToolName } from 'ai'
import type { DynamicToolUIPart, UIMessage } from 'ai'
import { isPartStreaming, isToolStreaming } from '@nuxt/ui/utils/ai'
import { getMergedParts } from '~/utils/ai'

const ChatToolChart = defineAsyncComponent(() => import('~/components/chat/tool/Chart.vue'))

defineProps<{
  message: UIMessage
}>()
</script>

<template>
  <template v-for="(part, index) in getMergedParts(message.parts)" :key="`${message.id}-${part.type}-${index}`">
    <UChatReasoning
      v-if="isReasoningUIPart(part)"
      :text="part.text"
      :streaming="isPartStreaming(part)"
      chevron="leading"
    >
      <ChatComark
        :markdown="part.text"
        :streaming="isPartStreaming(part)"
      />
    </UChatReasoning>

    <template v-else-if="isToolUIPart(part)">
      <ChatToolChart
        v-if="getToolName(part) === 'chart'"
        :invocation="{ ...(part as ChartUIToolInvocation) }"
      />
      <ChatToolWeather
        v-else-if="getToolName(part) === 'weather'"
        :invocation="{ ...(part as WeatherUIToolInvocation) }"
      />
      <ChatToolSendFile
        v-else-if="getToolName(part) === 'send_file'"
        :invocation="part as DynamicToolUIPart"
      />
      <UChatTool
        v-else-if="getToolName(part) === 'web_search' || getToolName(part) === 'google_search'"
        :text="isToolStreaming(part) ? 'Searching the web...' : 'Searched the web'"
        :suffix="getSearchQuery(part)"
        :streaming="isToolStreaming(part)"
        icon="i-lucide-search"
        variant="card"
        chevron="leading"
      >
        <ChatToolSources :sources="getSources(part)" />
      </UChatTool>

      <ChatToolGeneric
        v-else
        :invocation="part as DynamicToolUIPart"
      />
    </template>

    <template v-else-if="isTextUIPart(part)">
      <ChatComark
        v-if="message.role === 'assistant'"
        :markdown="part.text"
        :streaming="isPartStreaming(part)"
      />
      <p v-else class="whitespace-pre-wrap">
        {{ part.text }}
      </p>
    </template>
  </template>
</template>

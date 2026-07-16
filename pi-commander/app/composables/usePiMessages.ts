import type { DynamicToolUIPart, UIMessage } from 'ai'
import type { PiMessage, PiPart } from './usePiRemote'

function parseToolInput(input: string | undefined): unknown {
  if (!input) return undefined
  try {
    return JSON.parse(input)
  } catch {
    return input
  }
}

function parseToolOutput(output: string | undefined): unknown {
  if (!output) return undefined
  try {
    return JSON.parse(output)
  } catch {
    return output
  }
}

function isErrorOutput(output: string | undefined): { error: true, text: string } | { error: false } {
  if (output && output.startsWith('ERROR: ')) {
    return { error: true, text: output.slice(7) }
  }
  return { error: false }
}

function mapPartState(state: PiPart['state']): 'streaming' | 'done' | undefined {
  if (state === 'Streaming') return 'streaming'
  if (state === 'Done') return 'done'
  return undefined
}

function partToUIMessagePart(part: PiPart, index: number): UIMessage['parts'][number] | undefined {
  switch (part.type) {
    case 'text': {
      const text = part.text ?? ''
      if (!text) return undefined
      return { type: 'text', text, state: mapPartState(part.state) }
    }
    case 'thinking': {
      const text = part.text ?? ''
      if (!text) return undefined
      return { type: 'reasoning', text, state: mapPartState(part.state) }
    }
    case 'tool_call': {
      const name = part.name || 'unknown'
      const isStreaming = part.state === 'Streaming'
      return {
        type: 'dynamic-tool',
        toolName: name,
        toolCallId: `tool-${index}`,
        state: isStreaming ? 'input-streaming' : 'input-available',
        input: parseToolInput(part.args),
        details: part.details
      } as unknown as DynamicToolUIPart
    }
    case 'tool_result': {
      const name = part.name || 'unknown'
      const rawOutput = part.output || ''
      const error = isErrorOutput(rawOutput)
      if (error.error) {
        return {
          type: 'dynamic-tool',
          toolName: name,
          toolCallId: `tool-${index}`,
          state: 'output-error',
          input: undefined,
          errorText: error.text,
          details: part.details
        } as unknown as DynamicToolUIPart
      }
      return {
        type: 'dynamic-tool',
        toolName: name,
        toolCallId: `tool-${index}`,
        state: 'output-available',
        output: parseToolOutput(rawOutput),
        details: part.details
      } as unknown as DynamicToolUIPart
    }
    default:
      return undefined
  }
}

function findLastToolPart(parts: UIMessage['parts'], predicate: (part: DynamicToolUIPart) => boolean): DynamicToolUIPart | undefined {
  for (let i = parts.length - 1; i >= 0; i--) {
    const part = parts[i]
    if (part && part.type === 'dynamic-tool' && predicate(part)) {
      return part
    }
  }
  return undefined
}

function isInputState(state: string): boolean {
  return state === 'input-available' || state === 'input-streaming'
}

export function piMessageToUIMessage(msg: PiMessage): UIMessage {
  const parts: UIMessage['parts'] = []

  for (let i = 0; i < msg.parts.length; i++) {
    const piPart = msg.parts[i]!
    const uiPart = partToUIMessagePart(piPart, i)
    if (!uiPart) continue

    if (uiPart.type === 'dynamic-tool') {
      // Merge duplicate/placeholder tool_calls that share the same input
      // (e.g. a name="unknown" call followed by the real named call).
      if (isInputState(uiPart.state)) {
        const prev = parts[parts.length - 1]
        if (
          prev
          && prev.type === 'dynamic-tool'
          && isInputState(prev.state)
          && JSON.stringify(prev.input) === JSON.stringify(uiPart.input)
        ) {
          if (prev.toolName === 'unknown' && uiPart.toolName !== 'unknown') {
            prev.toolName = uiPart.toolName
          }
          // Keep the most recent streaming state.
          if (uiPart.state === 'input-streaming') {
            prev.state = 'input-streaming'
          }
          continue
        }
      }

      // Merge a tool_result with the most recent matching tool_call so the
      // input and output appear in a single collapsible ChatTool.
      if (uiPart.state === 'output-available' || uiPart.state === 'output-error') {
        const match = findLastToolPart(parts, part =>
          isInputState(part.state)
          && (part.toolName === uiPart.toolName || uiPart.toolName === 'unknown')
        )
        if (match) {
          match.state = uiPart.state
          if (uiPart.state === 'output-error') {
            match.errorText = uiPart.errorText
          } else {
            match.output = uiPart.output
          }
          continue
        }
      }
    }

    parts.push(uiPart)
  }

  return {
    id: msg.id,
    role: msg.role,
    parts
  } as UIMessage
}

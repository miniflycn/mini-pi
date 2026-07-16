import { parseJsonEventStream, uiMessageChunkSchema, type UIMessageChunk } from 'ai'

export type PartType = 'text' | 'thinking' | 'tool_call' | 'tool_result'

export interface PiPart {
  type: PartType
  text?: string
  state?: 'Streaming' | 'Done'
  name?: string
  args?: string
  output?: string
  details?: Record<string, unknown>
}

export interface PiMessage {
  id: string
  entry_id: string | null
  role: 'user' | 'assistant'
  parts: PiPart[]
}

export interface PiThread {
  id: string
  title: string | null
  preview: string | null
  session_file: string | null
  model: string | null
  thinking_level: string | null
  pinned: boolean
  metadata: Record<string, unknown>
  created_at: string
  updated_at: string
}

export interface PiThreadListResponse {
  threads: PiThread[]
  pagination: {
    page: number
    per_page: number
    total: number
    total_pages: number
  }
}

export interface PiStatus {
  enabled: boolean
  status: 'disabled' | 'starting' | 'running' | 'error'
  status_detail: string | { error: string }
  tunnel_url: string | null
  target_thread_id: string | null
}

export interface PiModel {
  provider: string
  id: string
  name: string
  thinking_level_map?: Record<string, string | null> | null
}

export interface PiWorkspace {
  id: string
  name: string
  path: string
  created_at: string
  updated_at: string
}

function toPiError(value: unknown): Error {
  if (value && typeof value === 'object' && 'error' in value) {
    return new Error(String((value as { error: unknown }).error))
  }
  return new Error('Unexpected response from remote API')
}

export function isBackendUnavailableError(err: unknown): boolean {
  if (err instanceof TypeError) return true
  if (err && typeof err === 'object' && 'name' in err && err.name === 'TypeError') return true

  const message = err instanceof Error ? err.message : String(err)
  return /failed to fetch|load failed|networkerror|could not connect|fetch failed/i.test(message)
}

export function usePiRemote() {
  const config = usePiRemoteConfig()

  function url(path: string): string {
    const base = config.baseUrl.value
    if (!base) throw new Error('Remote API is not configured')
    return `${base}${path}`
  }

  function headers(): Record<string, string> {
    const headers: Record<string, string> = {
      'Content-Type': 'application/json'
    }
    const token = config.token.value
    if (token) {
      headers.Authorization = `Bearer ${token}`
    }
    return headers
  }

  async function status(): Promise<PiStatus> {
    const res = await fetch(url('/status'), { headers: headers() })
    const data = await res.json()
    if (!res.ok) throw toPiError(data)
    return data as PiStatus
  }

  async function listThreads(params?: { page?: number, per_page?: number }): Promise<PiThreadListResponse> {
    let path = '/threads'
    const query = new URLSearchParams()
    if (params?.page !== undefined) query.set('page', String(params.page))
    if (params?.per_page !== undefined) query.set('per_page', String(params.per_page))
    if (query.toString()) path += `?${query.toString()}`

    const res = await fetch(url(path), { headers: headers() })
    const data = await res.json()
    if (!res.ok) throw toPiError(data)
    return data as PiThreadListResponse
  }

  async function createThread(modelId?: string, workspaceId?: string, signal?: AbortSignal): Promise<{ thread_id: string }> {
    const body: Record<string, unknown> = {}
    if (modelId) body.model_id = modelId
    if (workspaceId !== undefined) body.workspace_id = workspaceId

    const res = await fetch(url('/threads'), {
      method: 'POST',
      headers: headers(),
      body: Object.keys(body).length ? JSON.stringify(body) : undefined,
      signal
    })
    const data = await res.json()
    if (!res.ok) throw toPiError(data)
    return data as { thread_id: string }
  }

  async function openThread(id: string): Promise<{ thread_id: string }> {
    const res = await fetch(url(`/threads/${id}/open`), {
      method: 'POST',
      headers: headers()
    })
    const data = await res.json()
    if (!res.ok) throw toPiError(data)
    return data as { thread_id: string }
  }

  async function getMessages(id: string, sinceId?: string): Promise<PiMessage[]> {
    let path = `/threads/${id}/messages`
    if (sinceId) path += `?since_id=${encodeURIComponent(sinceId)}`
    const res = await fetch(url(path), { headers: headers() })
    const data = await res.json()
    if (!res.ok) throw toPiError(data)
    return data as PiMessage[]
  }

  async function sendMessageStream(id: string, message: string, signal?: AbortSignal): Promise<ReadableStream<UIMessageChunk>> {
    const res = await fetch(url(`/threads/${id}/message`), {
      method: 'POST',
      headers: headers(),
      body: JSON.stringify({ message }),
      signal
    })
    if (!res.ok) {
      const data = await res.json().catch(() => undefined)
      throw toPiError(data)
    }
    if (!res.body) {
      throw new Error('Remote API returned an empty stream')
    }
    return parseJsonEventStream({
      stream: res.body,
      schema: uiMessageChunkSchema
    }).pipeThrough(new TransformStream({
      transform(part, controller) {
        if (!part.success) {
          throw part.error
        }
        controller.enqueue(part.value)
      }
    }))
  }

  async function abortThread(id: string): Promise<{ status: string }> {
    const res = await fetch(url(`/threads/${id}/abort`), {
      method: 'POST',
      headers: headers()
    })
    const data = await res.json()
    if (!res.ok) throw toPiError(data)
    return data as { status: string }
  }

  async function setModel(id: string, modelId: string): Promise<{ status: string }> {
    const res = await fetch(url(`/threads/${id}/model`), {
      method: 'POST',
      headers: headers(),
      body: JSON.stringify({ model_id: modelId })
    })
    const data = await res.json()
    if (!res.ok) throw toPiError(data)
    return data as { status: string }
  }

  async function setThinkingLevel(id: string, thinkingLevel: string, signal?: AbortSignal): Promise<{ status: string }> {
    const res = await fetch(url(`/threads/${id}/thinking-level`), {
      method: 'POST',
      headers: headers(),
      body: JSON.stringify({ thinking_level: thinkingLevel }),
      signal
    })
    const data = await res.json()
    if (!res.ok) throw toPiError(data)
    return data as { status: string }
  }

  async function listModels(): Promise<PiModel[]> {
    const res = await fetch(url('/models'), { headers: headers() })
    const data = await res.json()
    if (!res.ok) throw toPiError(data)
    return (data.models ?? []) as PiModel[]
  }

  async function listWorkspaces(): Promise<PiWorkspace[]> {
    const res = await fetch(url('/workspaces'), { headers: headers() })
    const data = await res.json()
    if (!res.ok) throw toPiError(data)
    return (data.workspaces ?? []) as PiWorkspace[]
  }

  async function downloadFile(path: string, mimeType?: string): Promise<Blob> {
    const body: Record<string, unknown> = { path }
    if (mimeType) body.mime_type = mimeType

    const res = await fetch(url('/files/download'), {
      method: 'POST',
      headers: headers(),
      body: JSON.stringify(body)
    })
    if (!res.ok) {
      const data = await res.json().catch(() => undefined)
      throw toPiError(data)
    }
    return res.blob()
  }

  return {
    status,
    listThreads,
    createThread,
    openThread,
    getMessages,
    sendMessageStream,
    abortThread,
    setModel,
    setThinkingLevel,
    listModels,
    listWorkspaces,
    downloadFile
  }
}

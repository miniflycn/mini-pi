export interface ModelItem {
  label: string
  value: string
  icon: string
}

export function providerIcon(provider: string): string {
  const map: Record<string, string> = {
    'anthropic': 'i-simple-icons-anthropic',
    'openai': 'i-simple-icons-openai',
    'deepseek': 'i-simple-icons-deepseek',
    'kimi': 'i-simple-icons-moonshot',
    'moonshotai': 'i-simple-icons-moonshot',
    'kimi-coding': 'i-simple-icons-moonshot',
    'google': 'i-simple-icons-google',
    'xai': 'i-simple-icons-x',
    'groq': 'i-simple-icons-groq',
    'mistral': 'i-simple-icons-mistral',
    'cerebras': 'i-simple-icons-cerebras',
    'fireworks': 'i-simple-icons-fireworks',
    'together': 'i-simple-icons-together',
    'openrouter': 'i-simple-icons-openrouter',
    'nvidia': 'i-simple-icons-nvidia',
    'azure': 'i-simple-icons-microsoftazure',
    'azure-openai': 'i-simple-icons-microsoftazure',
    'azure-openai-responses': 'i-simple-icons-microsoftazure',
    'bedrock': 'i-simple-icons-amazonaws',
    'amazon-bedrock': 'i-simple-icons-amazonaws',
    'cloudflare': 'i-simple-icons-cloudflare',
    'cloudflare-ai-gateway': 'i-simple-icons-cloudflare',
    'cloudflare-workers-ai': 'i-simple-icons-cloudflare'
  }

  const normalized = provider.toLowerCase()
  return map[normalized] ?? 'i-simple-icons-openai'
}

export function toModelItems(models: Array<{ provider: string, id: string, name: string }>): ModelItem[] {
  return models.map(m => ({
    label: m.name,
    value: `${m.provider}:${m.id}`,
    icon: providerIcon(m.provider)
  }))
}

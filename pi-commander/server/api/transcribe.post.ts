import OpenAI from 'openai'

const MODEL = 'custom-xiaomi/mimo-v2.5-asr'

export default defineEventHandler(async (event) => {
  const body = await readBody(event)
  const dataUrl = body?.dataUrl

  if (typeof dataUrl !== 'string' || !dataUrl.startsWith('data:')) {
    throw createError({
      statusCode: 400,
      statusMessage: 'Invalid or missing dataUrl.'
    })
  }

  const CLOUDFLARE_API_KEY = process.env.CLOUDFLARE_API_KEY
  const CLOUDFLARE_ACCOUNT_ID = process.env.CLOUDFLARE_ACCOUNT_ID
  const CLOUDFLARE_GATEWAY_ID = process.env.CLOUDFLARE_GATEWAY_ID

  if (!CLOUDFLARE_API_KEY || !CLOUDFLARE_ACCOUNT_ID || !CLOUDFLARE_GATEWAY_ID) {
    throw createError({
      statusCode: 500,
      statusMessage: 'Missing Cloudflare gateway configuration.'
    })
  }

  // const { base64, format } = parseDataUrl(dataUrl)

  const openai = new OpenAI({
    apiKey: CLOUDFLARE_API_KEY,
    baseURL: `https://gateway.ai.cloudflare.com/v1/${CLOUDFLARE_ACCOUNT_ID}/${CLOUDFLARE_GATEWAY_ID}/compat`
  })

  try {
    const result = await openai.chat.completions.create({
      model: MODEL,
      messages: [
        {
          role: 'user',
          content: [
            {
              type: 'input_audio',
              input_audio: {
                data: dataUrl
              }
            }
          ]
        }
      ],
      asr_options: {
        language: 'auto'
      },
      stream: false
    } as unknown as OpenAI.Chat.ChatCompletionCreateParamsNonStreaming)

    const content = result.choices[0]?.message?.content
    if (typeof content !== 'string' || !content.trim()) {
      throw createError({
        statusCode: 502,
        statusMessage: 'Transcription returned empty content.'
      })
    }

    return { text: content.trim() }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err)
    throw createError({
      statusCode: 502,
      statusMessage: `Transcription failed: ${message}`
    })
  }
})

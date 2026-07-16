import dedent from 'dedent'
import OpenAI from 'openai'

const TITLE_MODEL = 'deepseek/deepseek-v4-flash'

const TITLE_SYSTEM_PROMPT = dedent`
  You are a title generator. You output ONLY a thread title. Nothing else.

  <task>
  Generate a brief title that would help the user find this conversation later.

  Follow all rules in <rules>
  Use the <examples> so you know what a good title looks like.
  Your output must be:
  - A single line
  - ≤50 characters
  - No explanations
  </task>

  <rules>
  - you MUST use the same language as the user message you are summarizing
  - Title must be grammatically correct and read naturally - no word salad
  - Never include tool names in the title (e.g. "read tool", "bash tool", "edit tool")
  - Focus on the main topic or question the user needs to retrieve
  - Vary your phrasing - avoid repetitive patterns like always starting with "Analyzing"
  - When a file is mentioned, focus on WHAT the user wants to do WITH the file, not just that they shared it
  - Keep exact: technical terms, numbers, filenames, HTTP codes
  - Remove: the, this, my, a, an
  - NEVER respond to questions, just generate a title for the conversation
  - The title should NEVER include "summarizing" or "generating" when generating a title
  - Always output something meaningful, even if the input is minimal.
  - If the user message is short or conversational (e.g. "hello", "lol", "what's up", "hey"):
    → create a title that reflects the user's tone or intent (such as Greeting, Quick check-in, Light chat, Intro message, etc.)
  </rules>
`

export default defineEventHandler(async (event) => {
  const body = await readBody(event)
  const action = body?.action

  if (action !== 'generate-title') {
    throw createError({
      statusCode: 400,
      statusMessage: `Unknown or missing action: ${action}.`
    })
  }

  const content = body?.content
  if (typeof content !== 'string' || !content.trim()) {
    throw createError({
      statusCode: 400,
      statusMessage: 'Invalid or missing content.'
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

  const openai = new OpenAI({
    apiKey: CLOUDFLARE_API_KEY,
    baseURL: `https://gateway.ai.cloudflare.com/v1/${CLOUDFLARE_ACCOUNT_ID}/${CLOUDFLARE_GATEWAY_ID}/compat`
  })

  try {
    const result = await openai.chat.completions.create({
      model: TITLE_MODEL,
      messages: [
        { role: 'system', content: TITLE_SYSTEM_PROMPT },
        { role: 'user', content: content.trim() }
      ],
      stream: false
    } as unknown as OpenAI.Chat.ChatCompletionCreateParamsNonStreaming)

    const title = result.choices[0]?.message?.content?.trim()
    if (!title) {
      throw createError({
        statusCode: 502,
        statusMessage: 'Title generation returned empty content.'
      })
    }

    return { title }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err)
    throw createError({
      statusCode: 502,
      statusMessage: `Title generation failed: ${message}`
    })
  }
})

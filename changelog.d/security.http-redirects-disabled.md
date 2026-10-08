- **Model clients no longer follow HTTP redirects** (`adk-anthropic`, `adk-gemini`,
  `adk-model`): the Anthropic client, the Gemini Studio and Vertex backends, and the
  OpenAI-compatible, Azure OpenAI, OpenAI Responses (generation), Azure AI Inference and
  DeepSeek clients return a 3xx response as an error instead of re-sending the request and
  its credentials to another host. Proxies configured through `HTTP_PROXY`/`HTTPS_PROXY`
  are unaffected. The Groq, OpenRouter and Conversations clients still follow redirects.

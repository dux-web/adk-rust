Request adapters for OpenAI Chat Completions and Responses now receive the complete
merged HTTP headers, including configured defaults. Explicit request headers take
precedence over defaults. Headers removed by an adapter are not added back by the
HTTP client, allowing custom authentication without placeholder credentials.

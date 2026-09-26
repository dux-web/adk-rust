# OpenCode Go

Set `OPENCODE_API_KEY`, then run `cargo run` from this directory. Running the example sends a model request using the Go account's quota.

The application supplies its own user agent and stable conversation ID. Reuse that ID for auxiliary requests in the same conversation. Start a different ID for a new conversation.

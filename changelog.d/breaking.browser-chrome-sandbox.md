- **Chrome starts with its sandbox on** (`adk-browser`): `BrowserSession` no longer adds
  `--no-sandbox` to every Chrome session. Containers that run Chrome as root opt in with
  `BrowserConfig::add_arg("--no-sandbox")`; `chrome_options` cannot replace the argument list.
  `BrowserConfig` gains the public fields `require_explicit_start` and `chrome_options`, so
  struct-literal construction needs `..Default::default()`.

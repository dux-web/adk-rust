- **Host-managed browser sessions** (`adk-browser`): `BrowserConfig::require_explicit_start`
  makes tools wait for the host to call `BrowserSession::start()`, which now keeps a live
  session and replaces one that no longer responds; `chrome_option` passes Chrome options
  such as `binary` and `prefs`.

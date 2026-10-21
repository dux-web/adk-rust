`RunConfig::tool_confirmation_timeout` is now `Option<Duration>`. Direct field
assignments use `Some(duration)`; the existing duration builder remains unchanged.
The default is still a five-minute SDK timeout. `None` explicitly delegates the
deadline to the confirmation handler so a durable approval owner can arbitrate
expiry against commit without an independent SDK denial.

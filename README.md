# airs-config

Layered runtime configuration for AIRS. It is independent of any application,
module, or service framework.

```rust
use airs_config::{Config, ConfigHandler};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct ConfigData {
    server: Server,
}

#[derive(Deserialize, Serialize)]
struct Server {
    port: u16,
}

impl ConfigHandler for ConfigData {
    fn default_config() -> airs_config::Result<String> {
        Ok("[server]\nport = 8080".to_string())
    }

    fn read_config() -> airs_config::Result<Option<String>> {
        Ok(None)
    }

    fn write_config(_config: &str) -> airs_config::Result<()> {
        Ok(())
    }
}

let mut config = Config::<ConfigData>::new()?;
config.server.port = 9090;
config.save()?;
let port = config.server.port;
# Ok::<(), airs_config::ConfigError>(())
```

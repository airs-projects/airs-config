use std::{
    ops::{Deref, DerefMut},
    path::PathBuf,
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub type Result<T> = std::result::Result<T, ConfigError>;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write config file {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid TOML: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("failed to serialize config as TOML: {0}")]
    SerializeToml(#[from] toml::ser::Error),

    #[error("invalid config data: {0}")]
    Data(#[from] serde_json::Error),

    #[error("config source is not valid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),

    #[error("invalid config reference: {0}")]
    InvalidReference(String),

    #[error("missing config reference: {0}")]
    MissingReference(String),

    #[error("config reference cycle: {0}")]
    ReferenceCycle(String),
}

pub trait ConfigHandler: DeserializeOwned + Serialize {
    fn default_config() -> Result<String>;

    fn read_config() -> Result<Option<String>>;

    fn write_config(config: &str) -> Result<()>;
}

#[derive(Clone, Debug)]
pub struct Config<T> {
    data: T,
}

impl<T> Config<T>
where
    T: ConfigHandler,
{
    pub fn new() -> Result<Self> {
        let mut root = parse_toml(&T::default_config()?)?;

        if let Some(content) = T::read_config()? {
            merge_values(&mut root, parse_toml(&content)?);
        }

        let snapshot = root.clone();
        resolve_value(&mut root, &snapshot)?;
        Ok(Self {
            data: serde_json::from_value(root)?,
        })
    }

    pub fn into_inner(self) -> T {
        self.data
    }

    pub fn save(&self) -> Result<()> {
        T::write_config(&toml::to_string_pretty(&self.data)?)
    }
}

impl<T> Deref for Config<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T> DerefMut for Config<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

fn parse_toml(content: &str) -> Result<Value> {
    Ok(serde_json::to_value(toml::from_str::<toml::Value>(
        content,
    )?)?)
}

fn merge_values(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                if let Some(base_value) = base.get_mut(&key) {
                    merge_values(base_value, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

fn resolve_value(value: &mut Value, root: &Value) -> Result<()> {
    match value {
        Value::String(text) => *text = resolve_string(text, root, &mut Vec::new())?,
        Value::Array(values) => {
            for value in values {
                resolve_value(value, root)?;
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                resolve_value(value, root)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn resolve_string(input: &str, root: &Value, stack: &mut Vec<String>) -> Result<String> {
    let mut result = input.to_string();

    while let Some(start) = result.find("${") {
        let end = result[start + 2..]
            .find('}')
            .map(|offset| start + 2 + offset)
            .ok_or_else(|| ConfigError::InvalidReference(result[start..].to_string()))?;
        let token = &result[start + 2..end];
        let path = reference_path(token)?;

        if stack.contains(&path) {
            return Err(ConfigError::ReferenceCycle(path));
        }

        let value = root
            .pointer(&path)
            .ok_or_else(|| ConfigError::MissingReference(token.to_string()))?;
        stack.push(path);
        let replacement = match value {
            Value::String(value) => resolve_string(value, root, stack)?,
            Value::Number(value) => value.to_string(),
            Value::Bool(value) => value.to_string(),
            _ => return Err(ConfigError::InvalidReference(token.to_string())),
        };
        stack.pop();

        result.replace_range(start..=end, &replacement);
    }

    Ok(result)
}

fn reference_path(token: &str) -> Result<String> {
    if token.starts_with('/') {
        validate_pointer(token)?;
        return Ok(token.to_string());
    }
    if token.is_empty() || token.split('.').any(str::is_empty) {
        return Err(ConfigError::InvalidReference(token.to_string()));
    }

    Ok(token.split('.').fold(String::new(), |mut path, segment| {
        path.push('/');
        path.push_str(&segment.replace('~', "~0").replace('/', "~1"));
        path
    }))
}

fn validate_pointer(path: &str) -> Result<()> {
    for segment in path[1..].split('/') {
        let mut chars = segment.chars();
        while let Some(character) = chars.next() {
            if character == '~' && !matches!(chars.next(), Some('0' | '1')) {
                return Err(ConfigError::InvalidReference(path.to_string()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct ConfigData {
        server: Server,
        client: Client,
    }

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct Server {
        host: String,
        port: u16,
    }

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct Client {
        endpoint: String,
    }

    impl ConfigHandler for ConfigData {
        fn default_config() -> Result<String> {
            Ok(r#"
                [server]
                host = "localhost"
                port = 8080

                [client]
                endpoint = "http://${server.host}:${server.port}"
                "#
            .to_string())
        }

        fn read_config() -> Result<Option<String>> {
            read_optional_file(config_path())
        }

        fn write_config(config: &str) -> Result<()> {
            let path = config_path();
            fs::write(&path, config).map_err(|source| ConfigError::Write { path, source })
        }
    }

    impl ConfigHandler for Value {
        fn default_config() -> Result<String> {
            Ok("first = '${second}'\nsecond = '${first}'".to_string())
        }

        fn read_config() -> Result<Option<String>> {
            Ok(None)
        }

        fn write_config(_config: &str) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn initializes_typed_data_from_merged_files() {
        let config_path = config_path();
        fs::write(&config_path, "[server]\nport = 9090").unwrap();

        let mut config = Config::<ConfigData>::new().unwrap();

        assert_eq!(
            *config,
            ConfigData {
                server: Server {
                    host: "localhost".to_string(),
                    port: 9090,
                },
                client: Client {
                    endpoint: "http://localhost:9090".to_string(),
                },
            }
        );

        config.server.port = 7070;
        config.save().unwrap();
        assert!(
            fs::read_to_string(&config_path)
                .unwrap()
                .contains("port = 7070")
        );
        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn reports_invalid_typed_data() {
        #[derive(Deserialize, Serialize)]
        struct InvalidConfigData {
            #[serde(rename = "required")]
            _required: u32,
        }

        impl ConfigHandler for InvalidConfigData {
            fn default_config() -> Result<String> {
                Ok("other = 1".to_string())
            }

            fn read_config() -> Result<Option<String>> {
                Ok(None)
            }

            fn write_config(_config: &str) -> Result<()> {
                Ok(())
            }
        }

        let result = Config::<InvalidConfigData>::new();

        assert!(matches!(result, Err(ConfigError::Data(_))));
    }

    #[test]
    fn detects_reference_cycles() {
        let result = Config::<Value>::new();

        assert!(matches!(result, Err(ConfigError::ReferenceCycle(_))));
    }

    #[test]
    fn version_matches_package_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }

    fn config_path() -> PathBuf {
        std::env::temp_dir().join(format!("airs-config-{}-config.toml", std::process::id()))
    }

    fn read_optional_file(path: PathBuf) -> Result<Option<String>> {
        match fs::read_to_string(&path) {
            Ok(content) => Ok(Some(content)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(ConfigError::Read { path, source }),
        }
    }
}

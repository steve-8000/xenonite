use anyhow::{Context, Result};
use serde::Deserialize;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

const DEFAULT_PORT: u16 = 8700;
const DEFAULT_LLM_URL: &str = "http://127.0.0.1:7777/v1";
const DEFAULT_LLM_MODEL: &str = "mlx-community/gemma-4-12B-it-qat-4bit";
const DEFAULT_EMBED_URL: &str = "http://127.0.0.1:7778/v1";
const DEFAULT_EMBED_MODEL: &str = "default";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub port: u16,
    pub data_dir: PathBuf,
    pub rocky: RockyConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RockyConfig {
    pub llm_base_url: String,
    pub llm_model: String,
    pub llm_api_key: String,
    pub embed_base_url: String,
    pub embed_model: String,
    pub embed_api_key: String,
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    port: Option<u16>,
    data_dir: Option<String>,
    llm_url: Option<String>,
    llm_model: Option<String>,
    llm_key: Option<String>,
    embed_url: Option<String>,
    embed_model: Option<String>,
    embed_key: Option<String>,
}

impl Config {
    pub fn load() -> Result<Self> {
        let file_config = read_file_config()?;
        let home = home_dir();
        let data_dir = env::var("XENONITE_DATA_DIR")
            .ok()
            .or(file_config.data_dir)
            .map(interpolate_env)
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share/xenonite"));

        fs::create_dir_all(&data_dir)
            .with_context(|| format!("create data dir {}", data_dir.display()))?;

        Ok(Self {
            port: env::var("XENONITE_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .or(file_config.port)
                .unwrap_or(DEFAULT_PORT),
            data_dir,
            rocky: RockyConfig {
                llm_base_url: env_value("ROCKY_LLM_URL", file_config.llm_url, DEFAULT_LLM_URL),
                llm_model: env_value("ROCKY_LLM_MODEL", file_config.llm_model, DEFAULT_LLM_MODEL),
                llm_api_key: env_value("ROCKY_LLM_KEY", file_config.llm_key, "x"),
                embed_base_url: env_value(
                    "ROCKY_EMBED_URL",
                    file_config.embed_url,
                    DEFAULT_EMBED_URL,
                ),
                embed_model: env_value(
                    "ROCKY_EMBED_MODEL",
                    file_config.embed_model,
                    DEFAULT_EMBED_MODEL,
                ),
                embed_api_key: env_value("ROCKY_EMBED_KEY", file_config.embed_key, "x"),
            },
        })
    }
}

fn read_file_config() -> Result<FileConfig> {
    let default_path = home_dir().join(".config/xenonite/xenonite.toml");
    let path = env::var("XENONITE_CONFIG")
        .map(PathBuf::from)
        .unwrap_or(default_path);

    if !Path::new(&path).exists() {
        return Ok(FileConfig::default());
    }

    let raw =
        fs::read_to_string(&path).with_context(|| format!("read config {}", path.display()))?;
    let interpolated = interpolate_env(raw);
    toml::from_str(&interpolated).with_context(|| format!("parse config {}", path.display()))
}

fn env_value(key: &str, file_value: Option<String>, fallback: &str) -> String {
    env::var(key)
        .ok()
        .or(file_value)
        .map(interpolate_env)
        .unwrap_or_else(|| fallback.to_string())
}

fn home_dir() -> PathBuf {
    env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn interpolate_env(value: String) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value.as_str();

    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        rest = &rest[start + 2..];

        if let Some(end) = rest.find('}') {
            let key = &rest[..end];
            output.push_str(&env::var(key).unwrap_or_default());
            rest = &rest[end + 1..];
        } else {
            output.push_str("${");
            output.push_str(rest);
            return output;
        }
    }

    output.push_str(rest);
    output
}

#[cfg(test)]
mod tests {
    use super::interpolate_env;

    #[test]
    fn interpolate_env_replaces_known_variables_and_drops_missing_values() {
        std::env::set_var("XENONITE_TEST_INTERPOLATE", "ok");

        assert_eq!(
            interpolate_env("${XENONITE_TEST_INTERPOLATE}:${XENONITE_TEST_MISSING}".to_string()),
            "ok:"
        );
    }
}

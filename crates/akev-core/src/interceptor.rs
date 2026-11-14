//! Interceptor descriptors shared between `akev-intercept`, the API and the UI.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InterceptorCategory {
    Browser,
    Terminal,
    Runtime,
    Mobile,
    Container,
    System,
    Manual,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterceptorInfo {
    /// Stable id, e.g. `chrome`, `firefox`, `fresh-terminal`, `existing-terminal`, `android-adb`.
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: InterceptorCategory,
    /// Search keywords.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Icon key for the UI (`chrome`, `firefox`, `terminal`, `android`, `docker`, ...).
    pub icon: String,
    /// Supported on this OS at all.
    pub supported: bool,
    /// Can be activated right now (e.g. browser installed, adb device connected).
    pub available: bool,
    /// Currently active (e.g. a browser window is open and proxied).
    pub active: bool,
    /// Why it's unavailable, if not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    /// Detected version, if relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Interceptor-specific metadata (e.g. list of adb devices, docker containers, JVMs).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
    /// True if activation needs an options payload (the UI shows a config panel first).
    #[serde(default)]
    pub needs_config: bool,
}

/// Result of activating an interceptor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationResult {
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Extra data for the UI (e.g. a shell snippet to paste, a QR code payload).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
}

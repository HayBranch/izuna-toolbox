//! WebDAV 同步：把备份文件 PUT 到服务器 / 从服务器 GET 回来。
//!
//! 与自动更新一样，网络实现 gate 到 `cfg(windows)`：应用只发布 Windows 版，
//! Linux 原生构建（CNB）不编译 reqwest，非 Windows 直接返回「不支持」。
//! 只接受 http(s) 地址；用户名/密码由 reqwest 按 Basic Auth 发送，不写入日志。

use serde::Deserialize;

/// 前端设置页填写的 WebDAV 连接信息（保存在 WebView localStorage，不写 conf.toml）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebdavConfig {
    /// 服务器根地址，如 https://dav.example.com/remote.php/dav/files/me
    pub base_url: String,
    pub username: String,
    pub password: String,
    /// 远程目录，如 izuna-toolbox（可多级，用 / 分隔）
    pub remote_dir: String,
}

pub async fn put(cfg: &WebdavConfig, file_name: &str, body: Vec<u8>) -> Result<String, String> {
    imp::put(cfg, file_name, body).await
}

pub async fn get(cfg: &WebdavConfig, file_name: &str) -> Result<String, String> {
    imp::get(cfg, file_name).await
}

fn validate_file_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
    {
        return Err("远程文件名不合法（不能为空，也不能包含路径分隔符）".into());
    }
    Ok(())
}

#[cfg(windows)]
mod imp {
    use std::time::Duration;

    use super::{validate_file_name, WebdavConfig};

    fn client() -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .user_agent("izuna-toolbox-sync")
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| format!("初始化 WebDAV 请求失败：{e}"))
    }

    /// 拼出最终文件 URL 和需要提前创建的目录 URL（多级目录逐级 MKCOL）。
    fn target_urls(
        cfg: &WebdavConfig,
        file_name: &str,
    ) -> Result<(reqwest::Url, Vec<reqwest::Url>), String> {
        validate_file_name(file_name)?;
        let base = cfg.base_url.trim();
        if base.is_empty() {
            return Err("请先填写 WebDAV 服务器地址".into());
        }
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err("WebDAV 服务器地址必须以 http:// 或 https:// 开头".into());
        }
        let mut current =
            reqwest::Url::parse(base).map_err(|e| format!("WebDAV 地址无效：{e}"))?;
        let mut dirs = Vec::new();
        for segment in cfg.remote_dir.trim().replace('\\', "/").split('/') {
            let segment = segment.trim();
            if segment.is_empty() {
                continue;
            }
            if segment == "." || segment == ".." {
                return Err("远程目录不能包含 . 或 ..".into());
            }
            let mut next = current.clone();
            {
                // path_segments_mut 会自动正确转义中文等字符，也不会把已有的 %xx 二次编码
                let mut segments = next
                    .path_segments_mut()
                    .map_err(|_| "WebDAV 地址不能作为目录使用".to_string())?;
                segments.pop_if_empty();
                segments.push(segment);
            }
            dirs.push(next.clone());
            current = next;
        }
        {
            let mut segments = current
                .path_segments_mut()
                .map_err(|_| "WebDAV 地址不能作为目录使用".to_string())?;
            segments.pop_if_empty();
            segments.push(file_name.trim());
        }
        Ok((current, dirs))
    }

    fn with_auth(
        req: reqwest::RequestBuilder,
        cfg: &WebdavConfig,
    ) -> reqwest::RequestBuilder {
        if cfg.username.trim().is_empty() {
            req
        } else {
            req.basic_auth(cfg.username.trim(), Some(&cfg.password))
        }
    }

    fn snippet(text: &str) -> String {
        let cleaned = text.trim().replace(['\r', '\n'], " ");
        let short: String = cleaned.chars().take(200).collect();
        if short.is_empty() {
            String::new()
        } else {
            format!("（{short}）")
        }
    }

    async fn check(
        resp: reqwest::Response,
        action: &str,
    ) -> Result<reqwest::Response, String> {
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let hint = match status.as_u16() {
            401 => "，请检查用户名 / 密码",
            403 => "，服务器拒绝访问",
            404 => "，请检查地址与目录是否存在",
            507 => "，服务器空间不足",
            _ => "",
        };
        Err(format!(
            "{action}失败：HTTP {status}{hint}{}",
            snippet(&body)
        ))
    }

    pub async fn put(
        cfg: &WebdavConfig,
        file_name: &str,
        body: Vec<u8>,
    ) -> Result<String, String> {
        let (url, dirs) = target_urls(cfg, file_name)?;
        let client = client()?;
        let mkcol = reqwest::Method::from_bytes(b"MKCOL")
            .map_err(|e| format!("初始化 WebDAV 请求失败：{e}"))?;

        // 逐级创建远程目录；405 = 已存在，直接继续。
        for dir in dirs {
            let resp = with_auth(client.request(mkcol.clone(), dir), cfg)
                .send()
                .await
                .map_err(|e| format!("连接 WebDAV 服务器失败：{e}"))?;
            if resp.status().as_u16() != 405 {
                check(resp, "创建远程目录").await?;
            }
        }

        let content_type = if file_name.to_ascii_lowercase().ends_with(".json") {
            "application/json; charset=utf-8"
        } else {
            "text/plain; charset=utf-8"
        };
        let resp = with_auth(
            client.put(url.clone()).header("Content-Type", content_type).body(body),
            cfg,
        )
        .send()
        .await
        .map_err(|e| format!("上传到 WebDAV 失败：{e}"))?;
        check(resp, "上传备份").await?;
        Ok(url.to_string())
    }

    pub async fn get(cfg: &WebdavConfig, file_name: &str) -> Result<String, String> {
        let (url, _) = target_urls(cfg, file_name)?;
        let client = client()?;
        let resp = with_auth(client.get(url), cfg)
            .send()
            .await
            .map_err(|e| format!("连接 WebDAV 服务器失败：{e}"))?;
        let resp = check(resp, "下载备份").await?;
        resp.text()
            .await
            .map_err(|e| format!("读取 WebDAV 备份内容失败：{e}"))
    }
}

#[cfg(not(windows))]
mod imp {
    use super::WebdavConfig;

    pub async fn put(
        _cfg: &WebdavConfig,
        _file_name: &str,
        _body: Vec<u8>,
    ) -> Result<String, String> {
        Err("WebDAV 同步仅支持 Windows".into())
    }

    pub async fn get(_cfg: &WebdavConfig, _file_name: &str) -> Result<String, String> {
        Err("WebDAV 同步仅支持 Windows".into())
    }
}

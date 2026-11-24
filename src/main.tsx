import React from "react";
import ReactDOM from "react-dom/client";
import { attachConsole } from "@tauri-apps/plugin-log";
import App from "./App";
import { applyThemeMode } from "./theme";
import "./index.css";

// 把 Rust 侧日志镜像到 WebView 控制台，方便用 DevTools 查看
attachConsole().catch(() => {});

// 渲染前先按系统外观上好主题类，避免首帧闪白（浮窗模式不受影响：背景强制透明）。
// 真实偏好由 conf.toml 提供，MainShell 加载设置后再应用。
// 旧版本把主题存在 localStorage，这里顺手清掉，避免留下无用的旧配置。
localStorage.removeItem("izuna.theme");
applyThemeMode();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

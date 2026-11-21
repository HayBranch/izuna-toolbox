import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { applyThemeMode } from "./theme";
import "./index.css";

// 渲染前先按偏好/系统外观上好主题类，避免首帧闪白（浮窗模式不受影响：背景强制透明）
applyThemeMode();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

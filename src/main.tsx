import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles/tokens.css";
import "./styles/components.css";
import "./styles/screens.css";
import { initTheme } from "./theme";

initTheme(); // 첫 화면부터 저장된 테마로 그린다
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

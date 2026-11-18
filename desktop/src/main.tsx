import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import PopupMenu from "./PopupMenu";
import BrowserGuard from "./BrowserLock";
import "./App.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    {new URLSearchParams(location.search).get("popup")==="menu"?<PopupMenu />:<BrowserGuard><App /></BrowserGuard>}
  </React.StrictMode>
);

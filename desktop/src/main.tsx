import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import OknoBrowser from "./OknoBrowser";
import "./App.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    {new URLSearchParams(location.search).has("okno")?<OknoBrowser/>:<App />}
  </React.StrictMode>
);

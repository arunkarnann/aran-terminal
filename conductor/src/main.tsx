import React from "react";
import ReactDOM from "react-dom/client";
import { ThemeProvider } from "./themes/ThemeContext";
import App from "./App";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider>
      <App />
    </ThemeProvider>
  </React.StrictMode>,
);

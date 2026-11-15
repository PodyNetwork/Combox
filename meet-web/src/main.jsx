import React from "react";
import { createRoot } from "react-dom/client";
import App from "./App.jsx";
import "@livekit/components-styles";
import "./style.css";
import "./lite.js";

createRoot(document.getElementById("root")).render(<App />);

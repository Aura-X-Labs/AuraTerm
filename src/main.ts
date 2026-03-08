import { createApp } from "vue";
import App from "./App.vue";

// Apply global styles
const style = document.createElement("style");
style.textContent = `
  html, body, #root {
    margin: 0;
    padding: 0;
    width: 100%;
    height: 100%;
    overflow: hidden;
    background-color: #1e1e1e;
    color: #ffffff;
    font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
  }
`;
document.head.appendChild(style);

createApp(App).mount("#root");
import DefaultTheme from "vitepress/theme";
import type { Theme } from "vitepress";
import ArchifyDiagram from "./components/ArchifyDiagram.vue";
import "./custom.css";

const theme: Theme = {
  extends: DefaultTheme,
  enhanceApp({ app }) {
    app.component("ArchifyDiagram", ArchifyDiagram);
  },
};

export default theme;

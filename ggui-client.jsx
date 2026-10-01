import React from "react";
import { createRoot } from "react-dom/client";
import { ThemeProvider } from "@ggui-ai/design/themes";
import Viewer from "evidence-generated-viewer";

const data = document.getElementById("document-props");
const root = document.getElementById("viewer");
if (!data || !root) throw new Error("Document viewer bootstrap is missing");
const props = JSON.parse(data.textContent);
createRoot(root).render(<ThemeProvider><Viewer {...props} /></ThemeProvider>);

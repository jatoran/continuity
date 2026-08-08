import { app, Menu } from "electron";

export function installApplicationMenu(send, currentTheme) {
  const modifier = process.platform === "darwin" ? "Cmd" : "Ctrl";
  const template = [
    ...(process.platform === "darwin" ? [{
      label: app.name,
      submenu: [
        { role: "about" },
        { type: "separator" },
        { role: "services" },
        { type: "separator" },
        { role: "hide" },
        { role: "hideOthers" },
        { role: "unhide" },
        { type: "separator" },
        { role: "quit" },
      ],
    }] : []),
    {
      label: "File",
      submenu: [
        item("New", `${modifier}+N`, "new"),
        item("Open…", `${modifier}+O`, "open"),
        { type: "separator" },
        item("Export", `${modifier}+S`, "save"),
        item("Export As…", `${modifier}+Shift+S`, "save-as"),
        { type: "separator" },
        process.platform === "darwin" ? { role: "close" } : { role: "quit" },
      ],
    },
    {
      label: "Edit",
      submenu: [
        { role: "undo" },
        { role: "redo" },
        { type: "separator" },
        { role: "cut" },
        { role: "copy" },
        { role: "paste" },
        { role: "selectAll" },
      ],
    },
    {
      label: "View",
      submenu: [
        themeItem("System theme", "system", currentTheme, send),
        themeItem("Light theme", "light", currentTheme, send),
        themeItem("Dark theme", "dark", currentTheme, send),
        { type: "separator" },
        { role: "resetZoom" },
        { role: "zoomIn" },
        { role: "zoomOut" },
        { role: "togglefullscreen" },
      ],
    },
    {
      label: "Help",
      submenu: [
        item("Check for updates", undefined, "check-updates"),
        item("Continuity releases", undefined, "open-releases"),
      ],
    },
  ];

  function item(label, accelerator, command) {
    return { label, accelerator, click: () => send("continuity:menu-command", command) };
  }

  Menu.setApplicationMenu(Menu.buildFromTemplate(template));
}

function themeItem(label, theme, currentTheme, send) {
  return {
    label,
    type: "radio",
    checked: theme === currentTheme,
    click: () => send("continuity:menu-command", `theme:${theme}`),
  };
}

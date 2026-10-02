(() => {
  const picker = document.getElementById("styleguide-picker");
  const view = document.getElementById("view-styleguide");
  const dialog = document.getElementById("styleguide-dialog");
  const actionStatus = document.getElementById("styleguide-action-status");
  const errors = document.getElementById("styleguide-registration-errors");
  const selections = new Map();
  let designId = null;

  function key() { return designId ? `frame-styleguide:${designId}` : "frame-last-styleguide"; }
  function textElement(tag, text) {
    const element = document.createElement(tag);
    element.textContent = text;
    return element;
  }
  function renderGuide() {
    const guide = Frame.activeStyleguide;
    if (!guide) { if (dialog.open) dialog.close(); return; }
    document.getElementById("styleguide-title").textContent = guide.name;
    document.getElementById("styleguide-summary").textContent = guide.summary;
    document.getElementById("styleguide-prompt").textContent = guide.prompt;
    document.getElementById("styleguide-document").textContent = guide.document;
    document.getElementById("styleguide-rules").replaceChildren(...guide.rules.map(rule => textElement("li", rule)));
    document.getElementById("styleguide-palette").replaceChildren(...guide.tokens.palette.map(color => {
      const swatch = document.createElement("div");
      const chip = document.createElement("span");
      chip.className = "guide-color";
      chip.style.backgroundColor = color.value;
      swatch.append(chip, textElement("strong", color.name), textElement("code", color.value));
      return swatch;
    }));
    document.getElementById("styleguide-tokens").replaceChildren(...["typography", "spacing", "geometry"].flatMap(name => [
      textElement("dt", name), textElement("dd", guide.tokens[name])
    ]));
    actionStatus.textContent = "";
  }
  function select(id) {
    Frame.activeStyleguide = Frame.styleguides.get(id) || null;
    picker.value = Frame.activeStyleguide?.id || "";
    picker.disabled = !Frame.styleguides.size;
    view.disabled = !Frame.activeStyleguide;
    renderGuide();
  }
  function restoreSelection() {
    let id = selections.get(key());
    if (!id) {
      try { id = localStorage.getItem(key()); } catch { /* selection persistence is optional */ }
    }
    select(Frame.styleguides.has(id) ? id : Frame.styleguides.keys().next().value);
  }
  function refresh() {
    picker.replaceChildren(...[...Frame.styleguides.values()].map(guide => {
      const option = textElement("option", guide.name);
      option.value = guide.id;
      return option;
    }));
    document.getElementById("styleguide-registration-notice").hidden = !Frame.styleguideErrors.length;
    errors.textContent = Frame.styleguideErrors.map(error => `${error.id ? error.id + ": " : ""}${error.message}`).join("\n");
    const previous = Frame.activeStyleguide;
    restoreSelection();
    if (Frame.activeStyleguide !== previous) window.dispatchEvent(new Event("frame:styleguide-selected"));
  }
  picker.addEventListener("change", () => {
    select(picker.value);
    selections.set(key(), picker.value);
    try { localStorage.setItem(key(), picker.value); } catch { /* still usable in this session */ }
    window.dispatchEvent(new Event("frame:styleguide-selected"));
  });
  view.addEventListener("click", () => { renderGuide(); dialog.showModal(); });
  document.getElementById("close-styleguide").addEventListener("click", () => dialog.close());
  document.getElementById("copy-styleguide-prompt").addEventListener("click", async () => {
    const guide = Frame.activeStyleguide;
    try {
      await navigator.clipboard.writeText(guide.prompt);
      actionStatus.textContent = "Prompt copied.";
    } catch {
      actionStatus.textContent = "Clipboard unavailable. Select the prompt text above or download the full guide.";
    }
  });
  document.getElementById("download-styleguide").addEventListener("click", () => {
    const guide = Frame.activeStyleguide;
    const url = URL.createObjectURL(new Blob([guide.document], {type: "text/markdown;charset=utf-8"}));
    const link = document.createElement("a");
    link.href = url;
    link.download = `${guide.id}.md`;
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  });
  window.addEventListener("frame:design-selected", event => {
    designId = event.detail.design.id;
    if (dialog.open) dialog.close();
    restoreSelection();
  });
  window.addEventListener("frame:styleguides-changed", refresh);
  refresh();
})();

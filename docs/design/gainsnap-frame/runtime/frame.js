// Frame designs are registered by ordinary scripts; no build step is needed.
(() => {
  const designs = new Map();
  const errors = [];

  function reject(design, message) {
    const id = typeof design?.id === "string" ? design.id : null;
    errors.push({id, message: id ? `${id}: ${message}` : message});
    window.dispatchEvent(new Event("frame:designs-changed"));
    return false;
  }

  function register(design) {
    if (!design || !/^[a-z0-9-]+$/.test(design.id || "") ||
        typeof design.name !== "string" || !design.name.trim() || !Number.isInteger(design.width) || !Number.isInteger(design.height) ||
        design.width < 1 || design.height < 1 || (design.revisionId !== undefined && (typeof design.revisionId !== "string" || !design.revisionId.trim())) || typeof design.paint !== "function") {
      return reject(design, "Frame.register expects an id, name, positive integer size, and paint function");
    }
    if (designs.has(design.id)) return reject(design, "Duplicate design id; the original design was kept");
    designs.set(design.id, design);
    window.dispatchEvent(new Event("frame:designs-changed"));
    return true;
  }

  const styleguides = new Map();
  const styleguideErrors = [];
  function registerStyleguide(guide) {
    let message = "";
    if (!guide || typeof guide.id !== "string" || !/^[a-z0-9-]+$/.test(guide.id) || typeof guide.name !== "string" || !guide.name.trim() ||
        typeof guide.summary !== "string" || !guide.summary.trim() || typeof guide.document !== "string" || !guide.document.trim() ||
        typeof guide.prompt !== "string" || !guide.prompt.trim() ||
        !Array.isArray(guide.rules) || !guide.rules.every(rule => typeof rule === "string") ||
        !guide.tokens || !Array.isArray(guide.tokens.palette) ||
        !guide.tokens.palette.every(color => typeof color?.name === "string" && /^#[0-9a-f]{6}$/i.test(color.value)) ||
        !["typography", "spacing", "geometry"].every(key => typeof guide.tokens[key] === "string")) {
      message = "Frame.registerStyleguide expects an id, name, summary, document, prompt, rules, and palette/typography/spacing/geometry tokens";
    } else if (styleguides.has(guide.id)) message = "Duplicate styleguide id; the original guide was kept";
    if (message) {
      styleguideErrors.push({id: typeof guide?.id === "string" ? guide.id : null, message});
      window.dispatchEvent(new Event("frame:styleguides-changed"));
      return false;
    }
    styleguides.set(guide.id, guide);
    window.dispatchEvent(new Event("frame:styleguides-changed"));
    return true;
  }

  window.Frame = {register, designs, errors, registerStyleguide, styleguides, styleguideErrors, activeStyleguide: null};
})();

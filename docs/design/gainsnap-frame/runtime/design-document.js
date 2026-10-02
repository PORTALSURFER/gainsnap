// Serializable design documents and atomic editing, shared by the browser and Node.
(function(root) {
  const clone = value => JSON.parse(JSON.stringify(value));
  const types = new Set(["container", "text", "button", "input", "line", "chart", "image"]);
  const finite = Number.isFinite;
  const fail = message => { throw new Error(message); };
  const validId = value => typeof value === "string" && /^[a-z0-9][a-z0-9_-]*$/i.test(value);
  const colors = {background: "#0D0F10", surface: "#121314", text: "#D5D8D6", muted: "#868C8A", accent: "#E86938", border: "#3E4443"};
  const tokens = {colors, spacing: {small: 8, medium: 16, large: 24}, typography: {family: "monospace", size: 14}};
  function validate(doc) {
    if (!doc || doc.version !== 1 || !validId(doc.id) || typeof doc.name !== "string" || !doc.name.trim() ||
        ![doc.width, doc.height].every(value => Number.isInteger(value) && value > 0) ||
        !validId(doc.revisionId) || !Array.isArray(doc.elements)) fail("Expected a version 1 design with id, name, positive integer size, revisionId, and elements");
    if (doc.elements.length > 10000) fail("Design exceeds the 10,000-element limit");
    const hexColor = value => typeof value === "string" && /^#[0-9a-f]{6}$/i.test(value);
    if (doc.backgroundGradient && (!hexColor(doc.backgroundGradient.inner) || !hexColor(doc.backgroundGradient.outer))) fail("Background gradient requires inner and outer hex colors");
    if (doc.backgroundTexture) {
      const t=doc.backgroundTexture,b=t.bounds||{x:0,y:0,width:doc.width,height:doc.height};
      if (!hexColor(t.color) || !finite(t.spacing) || t.spacing<2 || !finite(t.radius) || t.radius<=0 || t.radius>t.spacing/2 || !finite(t.opacity) || t.opacity<0 || t.opacity>1 ||
          !["x","y","width","height"].every(k=>finite(b[k])) || b.width<0 || b.height<0 ||
          Math.ceil(b.width/t.spacing)*Math.ceil(b.height/t.spacing)>100000) fail("Invalid or excessive background texture");
    }
    const ids = new Set();
    for (const node of doc.elements) {
      if (!node || !validId(node.id) || ids.has(node.id) || !types.has(node.type)) fail("Element IDs must be unique and element types must be supported");
      ids.add(node.id);
      if (!node.bounds || !["x", "y", "width", "height"].every(key => finite(node.bounds[key])) || node.bounds.width < 0 || node.bounds.height < 0) fail(node.id + ": invalid bounds");
      if (node.text !== undefined && typeof node.text !== "string") fail(node.id + ": text must be a string");
      if (node.style && (typeof node.style !== "object" || Array.isArray(node.style))) fail(node.id + ": invalid style");
      for (const key of ["fontSize", "lineWidth", "radius", "cutCorner", "opacity"]) if (node.style?.[key] !== undefined && !finite(node.style[key])) fail(node.id + ": invalid " + key);
      if (node.points && (!Array.isArray(node.points) || !node.points.every(point => Array.isArray(point) && point.length === 2 && point.every(finite)))) fail(node.id + ": invalid points");
      for (const layout of [node.responsive, ...(node.breakpoints || []).map(item => item.bounds)].filter(Boolean)) {
        for (const [key, value] of Object.entries(layout)) if (!["x", "y", "width", "height"].includes(key) || !(finite(value) || typeof value === "string" && /^-?\d+(\.\d+)?%$/.test(value))) fail(node.id + ": invalid responsive bounds");
      }
      if (node.breakpoints && (!Array.isArray(node.breakpoints) || !node.breakpoints.every(item => finite(item.maxWidth) && item.maxWidth > 0 && item.bounds))) fail(node.id + ": invalid breakpoints");
      if (["button", "input"].includes(node.type) && (typeof node.label !== "string" || !node.label.trim())) fail(node.id + ": controls need an accessible label");
      if (node.action && (node.action.type !== "set-state" || !validId(node.action.state))) fail(node.id + ": unsupported action");
    }
    const byId = new Map(doc.elements.map(node => [node.id, node]));
    for (const node of doc.elements) {
      let parent = node.parentId;
      const seen = new Set([node.id]);
      while (parent !== undefined && parent !== null) {
        if (!byId.has(parent)) fail(node.id + ": missing parent " + parent);
        if (seen.has(parent)) fail(node.id + ": hierarchy cycle");
        seen.add(parent);
        if (byId.get(parent).type !== "container") fail(node.id + ": parent must be a container");
        parent = byId.get(parent).parentId;
      }
      if (node.action && !Object.hasOwn(doc.states || {default: {}}, node.action.state)) fail(node.id + ": unknown action state");
    }
    if (doc.states && (typeof doc.states !== "object" || Array.isArray(doc.states) || !Object.hasOwn(doc.states, "default"))) fail("States require a default entry");
    for (const [state, overrides] of Object.entries(doc.states || {default: {}})) {
      if (!validId(state) || !overrides || typeof overrides !== "object" || Array.isArray(overrides)) fail("Invalid state");
      for (const [elementId, override] of Object.entries(overrides)) {
        if (!byId.has(elementId) || !override || typeof override !== "object" || Array.isArray(override) ||
            Object.keys(override).some(key => !["text", "visible", "disabled", "value", "style"].includes(key))) fail("Invalid state override for " + elementId);
        if (override.text !== undefined && typeof override.text !== "string") fail("State text must be a string");
        for (const key of ["visible", "disabled"]) if (override[key] !== undefined && typeof override[key] !== "boolean") fail("Invalid state " + key);
      }
    }
    if (doc.assets && (!Array.isArray(doc.assets) || !doc.assets.every(asset => validId(asset.id) && typeof asset.data === "string" && /^data:image\/(png|jpeg|webp);base64,/.test(asset.data)))) fail("Assets must be embedded PNG, JPEG, or WebP data URLs");
    const assetIds=new Set();
    for(const asset of doc.assets||[]) {if(assetIds.has(asset.id))fail("Duplicate asset ID");assetIds.add(asset.id);}
    for(const node of doc.elements)if(node.type==="image"&&!assetIds.has(node.assetId))fail(node.id+": missing image asset");
    return doc;
  }
  const imageCache=new Map();
  function ensureImage(data) {
    if(imageCache.has(data))return imageCache.get(data);
    const image=new Image();image.src=data;
    const promise=image.decode();promise.catch(()=>{});
    const entry={image,promise};imageCache.set(data,entry);return entry;
  }
  function prepareAssets(doc) {
    return Promise.all(doc.elements.filter(node=>node.type==="image").map(node=>ensureImage(doc.assets.find(asset=>asset.id===node.assetId).data).promise));
  }
  function layout(doc, viewport = {width: doc.width, height: doc.height}, state = "default") {
    if (!Object.hasOwn(doc.states || {default: {}}, state)) fail("Unknown state: " + state);
    const width = viewport.width, height = viewport.height;
    if (![width, height].every(value => Number.isInteger(value) && value > 0)) fail("Viewport needs positive integer dimensions");
    const resolved = new Map();
    const byId = new Map(doc.elements.map(node => [node.id, node]));
    function resolve(node) {
      if (resolved.has(node.id)) return resolved.get(node.id);
      const parent = node.parentId ? resolve(byId.get(node.parentId)) : null;
      const frame = parent?.bounds || {x: 0, y: 0, width, height};
      const overrides = (doc.states || {})[state]?.[node.id] || {};
      const patch = {...node.responsive};
      for (const breakpoint of [...(node.breakpoints || [])].sort((a, b) => b.maxWidth - a.maxWidth)) if (width <= breakpoint.maxWidth) Object.assign(patch, breakpoint.bounds);
      const bounds = {...node.bounds};
      for (const [key, value] of Object.entries(patch)) bounds[key] = typeof value === "string" ? parseFloat(value) / 100 * frame[["x", "width"].includes(key) ? "width" : "height"] : value;
      bounds.x += frame.x; bounds.y += frame.y;
      const result = {...node, ...overrides, style: {...node.style, ...overrides.style}, bounds,
        visible: node.visible !== false && overrides.visible !== false && (!parent || parent.visible)};
      resolved.set(node.id, result);
      return result;
    }
    return doc.elements.map(resolve);
  }
  function hitTest(elements, x, y) {
    return [...elements].reverse().filter(node => node.visible !== false && x >= node.bounds.x && y >= node.bounds.y &&
      x <= node.bounds.x + node.bounds.width && y <= node.bounds.y + node.bounds.height);
  }
  function edit(doc, operations) {
    if (!Array.isArray(operations)) fail("Operations must be an array");
    const next = clone(doc);
    for (const operation of operations) {
      const index = next.elements.findIndex(node => node.id === operation.id);
      if (operation.op === "create") {
        if (!operation.element || next.elements.some(node => node.id === operation.element.id)) fail("Cannot create duplicate or missing element");
        next.elements.push(clone(operation.element));
      } else if (operation.op === "update") {
        if (index < 0 || !operation.patch || Object.hasOwn(operation.patch, "id")) fail("Update needs an existing ID and cannot change it");
        const node = next.elements[index];
        next.elements[index] = {...node, ...clone(operation.patch), bounds: {...node.bounds, ...operation.patch.bounds}, style: {...node.style, ...operation.patch.style}};
      } else if (operation.op === "remove") {
        if (index < 0) fail("Cannot remove unknown element");
        const removed = new Set([operation.id]);
        let changed = true;
        while (changed) { changed = false; for (const node of next.elements) if (removed.has(node.parentId) && !removed.has(node.id)) { removed.add(node.id); changed = true; } }
        next.elements = next.elements.filter(node => !removed.has(node.id));
        for (const overrides of Object.values(next.states || {})) for (const id of removed) delete overrides[id];
      } else fail("Unknown operation: " + operation.op);
    }
    next.revisionId = "r-" + crypto.randomUUID();
    return validate(next);
  }
  function component(type, id, bounds, options = {}) {
    return {id, type, bounds: {...bounds}, ...options, ...(type === "button" || type === "input" ? {label: options.label || options.text || id} : {})};
  }
  const components = Object.fromEntries([...types].map(type => [type, (id, bounds, options) => component(type, id, bounds, options)]));
  function render(ctx, doc, options = {}) {
    const elements = layout(doc, options.viewport, options.state);
    const palette = {...colors, ...doc.tokens?.colors};
    ctx.fillStyle = doc.background || palette.background;
    if (doc.backgroundGradient) {
      const width=options.viewport?.width||doc.width,height=options.viewport?.height||doc.height;
      const gradient=ctx.createRadialGradient(width*0.35,height*0.65,0,width*0.35,height*0.65,Math.max(width,height)*0.8);
      gradient.addColorStop(0,doc.backgroundGradient.inner);gradient.addColorStop(1,doc.backgroundGradient.outer);
      ctx.fillStyle=gradient;
    }
    ctx.fillRect(0, 0, options.viewport?.width || doc.width, options.viewport?.height || doc.height);
    if (doc.backgroundTexture) {
      const t=doc.backgroundTexture,b=t.bounds||{x:0,y:0,width:doc.width,height:doc.height};
      ctx.save();ctx.globalAlpha=t.opacity;ctx.fillStyle=t.color;
      for(let row=0,y=b.y;y<b.y+b.height;row++,y+=t.spacing) {
        for(let x=b.x+(row%2?t.spacing/2:0);x<b.x+b.width;x+=t.spacing) {
          ctx.beginPath();ctx.arc(x,y,t.radius,0,Math.PI*2);ctx.fill();
        }
      }
      ctx.restore();
    }
    for (const node of elements) {
      if (!node.visible) continue;
      const b = node.bounds, s = node.style || {};
      ctx.save();
      ctx.globalAlpha = s.opacity ?? 1;
      ctx.lineWidth = s.lineWidth ?? 1;
      ctx.strokeStyle = palette[s.stroke] || s.stroke || palette.border;
      ctx.fillStyle = palette[s.fill] || s.fill || palette.surface;
      if (["container", "button", "input"].includes(node.type)) {
        ctx.beginPath();
        const radius = Math.max(0, Math.min(s.radius || 0, b.width / 2, b.height / 2));
        const cut = Math.max(0, Math.min(s.cutCorner || 0, b.width / 2, b.height / 2));
        if (node.points) {
          node.points.forEach(([x,y],i)=>ctx[i?"lineTo":"moveTo"](b.x+x,b.y+y));ctx.closePath();
        } else if (cut) {
          const x=b.x,y=b.y,right=x+b.width,bottom=y+b.height,r=Math.min(radius,cut);
          ctx.moveTo(x+r,y);ctx.lineTo(right-r,y);ctx.quadraticCurveTo(right,y,right,y+r);
          ctx.lineTo(right,bottom-cut);ctx.lineTo(right-cut,bottom);ctx.lineTo(x+r,bottom);
          ctx.quadraticCurveTo(x,bottom,x,bottom-r);ctx.lineTo(x,y+r);ctx.quadraticCurveTo(x,y,x+r,y);ctx.closePath();
        } else ctx.roundRect(b.x,b.y,b.width,b.height,radius);
        if (s.fillTop && s.fillBottom) {
          const gradient=ctx.createLinearGradient(0,b.y,0,b.y+b.height);
          gradient.addColorStop(0,palette[s.fillTop]||s.fillTop);gradient.addColorStop(1,palette[s.fillBottom]||s.fillBottom);
          ctx.fillStyle=gradient;
        }
        if (s.fill !== "none") ctx.fill();
        if (s.stroke !== "none") ctx.stroke();
      }
      if(node.type==="image") {
        const asset=doc.assets.find(asset=>asset.id===node.assetId);
        const entry=ensureImage(asset.data);
        if(entry.image.complete&&entry.image.naturalWidth)ctx.drawImage(entry.image,b.x,b.y,b.width,b.height);
      }
      if (["line", "chart"].includes(node.type)) {
        ctx.beginPath();
        const points = node.points || [[0, 0], [b.width, b.height]];
        points.forEach(([x, y], i) => ctx[i ? "lineTo" : "moveTo"](b.x + x * (node.type === "chart" ? b.width / (doc.elements.find(source => source.id === node.id).bounds.width || 1) : 1), b.y + y * (node.type === "chart" ? b.height / (doc.elements.find(source => source.id === node.id).bounds.height || 1) : 1))); ctx.stroke();
      }
      if (node.text !== undefined || node.type === "input") {
        ctx.fillStyle = palette[s.color] || s.color || palette.text;
        ctx.font = `${s.fontWeight || 400} ${s.fontSize || doc.tokens?.typography?.size || 14}px ${s.fontFamily || doc.tokens?.typography?.family || "monospace"}`;
        ctx.textBaseline = "top"; ctx.textAlign = s.textAlign || "left";
        const padding = s.padding ?? (["button", "input"].includes(node.type) ? 8 : 0);
        const x = s.textAlign === "center" ? b.x + b.width / 2 : s.textAlign === "right" ? b.x + b.width - padding : b.x + padding;
        const content = node.type === "input" ? options.values?.[node.id] ?? node.value ?? node.text ?? "" : node.text || "";
        const lines = String(content).split("\n");
        if (s.verticalAlign === "middle" && lines.length === 1) {
          ctx.textBaseline = "alphabetic";
          const metrics = ctx.measureText(lines[0]);
          const ascent = metrics.actualBoundingBoxAscent ?? (s.fontSize || 14) * 0.8;
          const descent = metrics.actualBoundingBoxDescent ?? (s.fontSize || 14) * 0.2;
          ctx.textBaseline = "alphabetic";
          ctx.fillText(lines[0], x, b.y + (b.height + ascent - descent) / 2);
        } else lines.forEach((line, i) => ctx.fillText(line, x, b.y + padding + i * (s.lineHeight || (s.fontSize || 14) * 1.4)));
      }
      ctx.restore();
    }
    return elements;
  }
  function inspect(doc, viewport, state) { return layout(doc, viewport, state).map(node => ({...node, children: doc.elements.filter(child => child.parentId === node.id).map(child => child.id)})); }
  const api = {prepareAssets, validate, layout, hitTest, edit, render, inspect, components, tokens, clone};
  if (typeof module !== "undefined") module.exports = api; else root.FrameDocument = api;
})(typeof window === "undefined" ? globalThis : window);

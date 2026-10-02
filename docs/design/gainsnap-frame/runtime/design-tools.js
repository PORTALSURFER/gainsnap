// Browser-facing operations on serializable design documents.
(() => {
  const documents=new Map(), viewports=new Map(), states=new Map(), values=new Map();
  const copy=FrameDocument.clone;
  let bridge=null, renderPromise=Promise.resolve();
  function requireDocument(id) {const doc=documents.get(id);if(!doc) throw new Error("No structured document for design: "+id);return doc;}
  function view(id) {const doc=requireDocument(id);return viewports.get(id)||{width:doc.width,height:doc.height};}
  function changed(id) {window.dispatchEvent(new CustomEvent("frame:design-updated",{detail:{id}}));}
  function registerDocument(input) {
    const doc=copy(FrameDocument.validate(input));
    if(Frame.designs.has(doc.id)) {Frame.errors.push({id:doc.id,message:doc.id+": Duplicate design id; the original design was kept"});window.dispatchEvent(new Event("frame:designs-changed"));return false;}
    documents.set(doc.id,doc);
    const result=Frame.register({id:doc.id,name:doc.name,width:doc.width,height:doc.height,revisionId:doc.revisionId,
      paint(ctx) {FrameDocument.render(ctx,documents.get(doc.id),{viewport:view(doc.id),state:states.get(doc.id)||"default",values:values.get(doc.id)});}});
    if(!result) documents.delete(doc.id);
    return result;
  }
  function updateDocument(input) {
    const doc=copy(FrameDocument.validate(input)), current=requireDocument(doc.id);
    if(doc.revisionId===current.revisionId&&JSON.stringify(doc)!==JSON.stringify(current)) throw new Error("Changed documents need a new revisionId");
    Frame.validateRevision?.(doc);
    documents.set(doc.id,doc);
    Object.assign(Frame.designs.get(doc.id),{name:doc.name,width:viewports.get(doc.id)?.width||doc.width,height:viewports.get(doc.id)?.height||doc.height,revisionId:doc.revisionId});
    if(!Object.hasOwn(doc.states||{default:{}},states.get(doc.id)||"default")) states.set(doc.id,"default");
    changed(doc.id);return copy(doc);
  }
  function setViewport(id,viewport) {
    requireDocument(id);
    if(!viewport||![viewport.width,viewport.height].every(n=>Number.isInteger(n)&&n>0&&n<=8192)) throw new Error("Viewport dimensions must be integers from 1 to 8192");
    viewports.set(id,{width:viewport.width,height:viewport.height});Object.assign(Frame.designs.get(id),viewport);changed(id);return copy(viewport);
  }
  function setState(id,state) {
    if(!Object.hasOwn(requireDocument(id).states||{default:{}},state)) throw new Error("Unknown state: "+state);
    states.set(id,state);changed(id);return state;
  }
  function setValue(id,elementId,value) {
    const node=requireDocument(id).elements.find(node=>node.id===elementId);if(!node||node.type!=="input") throw new Error("Unknown input: "+elementId);
    const inputs=values.get(id)||{};inputs[elementId]=String(value);values.set(id,inputs);changed(id);return inputs[elementId];
  }
  function inspect(id,options={}) {
    let nodes=FrameDocument.inspect(requireDocument(id),view(id),states.get(id)||"default");
    if(options.elementId) nodes=nodes.filter(node=>node.id===options.elementId);
    if(options.region) {const b=options.region;if(!["x","y","width","height"].every(k=>Number.isFinite(b[k]))||b.width<0||b.height<0) throw new Error("Invalid inspection region");
      nodes=nodes.filter(n=>n.bounds.x+n.bounds.width>=b.x&&n.bounds.x<=b.x+b.width&&n.bounds.y+n.bounds.height>=b.y&&n.bounds.y<=b.y+b.height);}
    for (const node of nodes) if (node.type === "input" && values.get(id)?.[node.id] !== undefined) node.value = values.get(id)[node.id];
    return copy(nodes);
  }
  async function exportImage({designId,feedback=false,scale=1}={}) {
    const design=Frame.designs.get(designId||bridge?.selectedId());if(!design) throw new Error("Unknown design");
    if(!Number.isFinite(scale)||scale<=0||scale>4||design.width*design.height*scale*scale>64000000) throw new Error("Invalid export scale or image too large");
    await document.fonts.ready;if(documents.has(design.id))await FrameDocument.prepareAssets(requireDocument(design.id));await renderPromise;
    const canvas=document.createElement("canvas");canvas.width=Math.round(design.width*scale);canvas.height=Math.round(design.height*scale);
    const ctx=canvas.getContext("2d");if(!ctx) throw new Error("Canvas unavailable");
    ctx.scale(scale,scale);design.paint(ctx,{styleguide:Frame.activeStyleguide});
    if(feedback) {
      if(bridge.selectedId()!==design.id) throw new Error("Select the design before exporting its feedback overlay");
      const marks=bridge.feedback();ctx.lineWidth=3;ctx.lineCap="round";ctx.strokeStyle="#ff3434";
      for(const line of marks.lines) {ctx.beginPath();line.points.forEach(([x,y],i)=>ctx[i?"lineTo":"moveTo"](x,y));ctx.stroke();}
      for(const note of marks.notes) {ctx.fillStyle="#362824";ctx.fillRect(note.anchor[0]-8,note.anchor[1]-8,220,50);ctx.fillStyle="#ffe2d4";ctx.font="12px monospace";ctx.fillText(note.text.split("\n")[0],note.anchor[0],note.anchor[1]+14,200);}
    }
    const blob=await new Promise((resolve,reject)=>canvas.toBlob(b=>b?resolve(b):reject(new Error("Image export failed")),"image/png"));
    return {blob,width:canvas.width,height:canvas.height,designId:design.id,revisionId:design.revisionId??null,scale,feedback};
  }
  function requireBridge() {if(!bridge) throw new Error("Frame UI is not ready");return bridge;}
  Frame.documents=documents;Frame.components=FrameDocument.components;Frame.tokens=FrameDocument.tokens;Frame.registerDocument=registerDocument;
  Frame.agent={
    listDesigns:()=>[...Frame.designs.values()].map(({id,name,width,height,revisionId})=>({id,name,width,height,revisionId:revisionId??null,structured:documents.has(id)})),
    getDocument:id=>copy(requireDocument(id)),
    selectDesign:id=>{if(!Frame.designs.has(id)) throw new Error("Unknown design");requireBridge().selectDesign(id);return id;},
    inspect, hitTest:(id,x,y)=>{if(![x,y].every(Number.isFinite)) throw new Error("Invalid point");return copy(FrameDocument.hitTest(inspect(id),x,y));},
    applyEdits:(id,operations)=>updateDocument(FrameDocument.edit(requireDocument(id),operations)),
    updateDocument,setViewport,setState,setValue,
    getValues: id => copy(values.get(id)||{}),
    setValues: (id, next) => {const doc=requireDocument(id);if(!next||typeof next!=="object"||Object.keys(next).some(key=>!doc.elements.some(n=>n.id===key&&n.type==="input")||typeof next[key]!=="string"))throw new Error("Invalid input values");values.set(id,copy(next));changed(id);},
getFeedback:()=>copy(requireBridge().feedback()),exportImage,
    renderReady:()=>Promise.all([document.fonts.ready,renderPromise]).then(()=>{if(!bridge?.isReady())throw new Error("Design is not render-ready");return {designId:bridge.selectedId(),ready:true};}),
    refreshDesign:(id,patch)=>{const current=Frame.designs.get(id);if(!current||documents.has(id)) throw new Error("Use updateDocument for structured designs");
      const next={...current,...patch,id};if(typeof next.paint!=="function"||![next.width,next.height].every(n=>Number.isInteger(n)&&n>0)||typeof next.name!=="string"||!next.name.trim()) throw new Error("Invalid design update");
      Object.assign(current,next);changed(id);return id;}
  };
  Frame.connectUI=api=>{bridge=api;};
  Frame.markRendering=promise=>{renderPromise=promise;promise.catch(()=>{});};
  Frame.currentViewport=id=>copy(view(id));
  Frame.currentState=id=>states.get(id)||"default";
})();

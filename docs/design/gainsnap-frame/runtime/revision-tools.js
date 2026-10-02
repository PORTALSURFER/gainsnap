// Revision snapshots and portable structured projects. Canvas callbacks remain code.
(() => {
  const archives=new Map(),unsaved=new Set(),blocked=new Set();
  const copy=FrameDocument.clone;
  const originalRegister=Frame.registerDocument;
  const key=id=>"frame-project-v1:"+id;
  function notify() {
    const status=document.getElementById("project-save-notice");
    if(status)status.textContent=unsaved.size?(blocked.size?"Unreadable saved project data is protected. Download the project before closing.":"Project changes are unsaved. Download the project before closing or reloading, or retry project saving."):"";
    const retry=document.getElementById("retry-project-save");if(retry)retry.hidden=!unsaved.size;
  }
  function persist(id) {
    try {if(blocked.has(id))throw new Error("Saved project is unreadable and protected");localStorage.setItem(key(id),JSON.stringify(archives.get(id)));unsaved.delete(id);}
    catch {unsaved.add(id);}
    notify();
  }
  function record(doc, sourceRevisionId) {
    const archive=archives.get(doc.id)||{version:1,designId:doc.id,sourceRevisionId:sourceRevisionId||doc.revisionId,currentRevisionId:doc.revisionId,revisions:[]};
    const existing=archive.revisions.find(r=>r.revisionId===doc.revisionId);
    if(existing&&JSON.stringify(existing.document)!==JSON.stringify(doc))throw new Error("A revision ID cannot identify two different documents");
    if(!existing)archive.revisions.push({revisionId:doc.revisionId,capturedAt:new Date().toISOString(),parentRevisionId:archive.currentRevisionId===doc.revisionId?null:archive.currentRevisionId,document:copy(doc)});
    archive.currentRevisionId=doc.revisionId;
    if(sourceRevisionId)archive.sourceRevisionId=sourceRevisionId;
    if(Frame.documents.has(doc.id)) archive.context={viewport:Frame.currentViewport(doc.id),state:Frame.currentState(doc.id),values:Frame.agent.getValues?.(doc.id)||{}};
    archives.set(doc.id,archive);
    return archive;
  }
  function validateArchive(archive) {
    if(!archive||archive.version!==1||typeof archive.designId!=="string"||!Array.isArray(archive.revisions)||!archive.revisions.length)throw new Error("Invalid revision archive");
    const ids=new Set();
    for(const revision of archive.revisions) {
      FrameDocument.validate(revision.document);
      if(revision.document.id!==archive.designId||revision.revisionId!==revision.document.revisionId||ids.has(revision.revisionId)||!Number.isFinite(Date.parse(revision.capturedAt)))throw new Error("Invalid revision snapshot");
      ids.add(revision.revisionId);
    }
    if(!ids.has(archive.currentRevisionId))throw new Error("Missing current revision");
    return archive;
  }
  Frame.registerDocument=input=>{
    let archive=null;
    try {const saved=localStorage.getItem(key(input.id));if(saved)archive=copy(validateArchive(JSON.parse(saved)));}
    catch {blocked.add(input.id);}
    if(archive)archives.set(input.id,archive);
    const document=archive?.sourceRevisionId===input.revisionId?archive.revisions.find(r=>r.revisionId===archive.currentRevisionId).document:input;
    const context=archive?.sourceRevisionId===input.revisionId?copy(archive.context||null):null;
    const result=originalRegister(document);
    if(result&&context) {
      if(context.viewport&&[context.viewport.width,context.viewport.height].every(n=>Number.isInteger(n)&&n>0&&n<=8192))Frame.agent.setViewport(input.id,context.viewport);
      if(Object.hasOwn(document.states||{default:{}},context.state))Frame.agent.setState(input.id,context.state);
      if(context.values&&typeof context.values==="object") {
        const values=Object.fromEntries(Object.entries(context.values).filter(([id,value])=>typeof value==="string"&&document.elements.some(n=>n.id===id&&n.type==="input")));
        Frame.agent.setValues?.(input.id,values);
      }
    }
    if(result){record(document,input.revisionId);persist(input.id);refresh();}
    return result;
  };
  function currentId(){return document.getElementById("design-picker").value;}
  function revision(id,revisionId) {
    const found=archives.get(id)?.revisions.find(r=>r.revisionId===revisionId);if(!found)throw new Error("Unknown revision: "+revisionId);return found;
  }
  function compare(id,beforeId,afterId) {
    const before=revision(id,beforeId).document,after=revision(id,afterId).document;
    const old=new Map(before.elements.map(n=>[n.id,n])),next=new Map(after.elements.map(n=>[n.id,n]));
    return {designId:id,beforeRevisionId:beforeId,afterRevisionId:afterId,
      added:[...next.keys()].filter(id=>!old.has(id)),removed:[...old.keys()].filter(id=>!next.has(id)),
      changed:[...next.keys()].filter(id=>old.has(id)).map(id=>({id,fields:[...new Set([...Object.keys(old.get(id)),...Object.keys(next.get(id))])].filter(k=>JSON.stringify(old.get(id)[k])!==JSON.stringify(next.get(id)[k]))})).filter(item=>item.fields.length),
      settings:["width","height","background","backgroundGradient","backgroundTexture","tokens","states"].filter(k=>JSON.stringify(before[k])!==JSON.stringify(after[k]))};
  }
  function exportProject(id=currentId()) {
    if(!Frame.documents.has(id))throw new Error("Portable export requires a structured design; custom Canvas code stays in its source files");
    if(currentId()!==id)throw new Error("Select the design before exporting its feedback");
    const doc=Frame.agent.getDocument(id),guide=Frame.activeStyleguide;
    return {format:"frame-project",version:1,archive:copy(archives.get(id)),feedback:Frame.agent.getFeedback(),styleguide:guide?copy(guide):null,
      viewport:Frame.currentViewport(id),state:Frame.currentState(id),values:Frame.agent.getValues?.(id)||{},assets:copy(doc.assets||[])};
  }
  function validateProject(project) {
    if(JSON.stringify(project).length>32*1024*1024)throw new Error("Project exceeds the 32 MB import limit");
    if(project?.format!=="frame-project"||project.version!==1)throw new Error("Unsupported project format");
    validateArchive(project.archive);
    const doc=project.archive.revisions.find(r=>r.revisionId===project.archive.currentRevisionId).document;
    FrameFeedback.validate(project.feedback,{id:doc.id});
    if(project.feedback.width!==project.viewport?.width||project.feedback.height!==project.viewport?.height||
      ![project.viewport?.width,project.viewport?.height].every(n=>Number.isInteger(n)&&n>0&&n<=8192))throw new Error("Project viewport and feedback dimensions do not match");
    if(!Object.hasOwn(doc.states||{default:{}},project.state))throw new Error("Project has an unknown interface state");
    if(!project.values||typeof project.values!=="object"||Object.keys(project.values).some(id=>!doc.elements.some(n=>n.id===id&&n.type==="input")||typeof project.values[id]!=="string"))throw new Error("Invalid prototype input values");
    if(project.styleguide) {
      const guide=project.styleguide;
      if(typeof guide.id!=="string"||!/^[-a-z0-9]+$/.test(guide.id)||!["name","summary","prompt","document"].every(k=>typeof guide[k]==="string"&&guide[k].trim())||
        !Array.isArray(guide.rules)||!guide.rules.every(v=>typeof v==="string")||!guide.tokens||!Array.isArray(guide.tokens.palette)||
        !guide.tokens.palette.every(c=>typeof c?.name==="string"&&/^#[\da-f]{6}$/i.test(c.value))||!["typography","spacing","geometry"].every(k=>typeof guide.tokens[k]==="string"))throw new Error("Invalid bundled styleguide");
    }
    if(JSON.stringify(project.assets||[])!==JSON.stringify(doc.assets||[]))throw new Error("Bundled assets differ from the current document");
    return project;
  }
  function importProject(input) {
    const project=copy(validateProject(input)),id=project.archive.designId;
    if(Frame.designs.has(id))throw new Error("Project ID already exists; import into a fresh browser or rename the project first");
    // Validate every dependency before changing any registry.
    if(project.styleguide&&Frame.styleguides.has(project.styleguide.id)&&JSON.stringify(Frame.styleguides.get(project.styleguide.id))!==JSON.stringify(project.styleguide))throw new Error("Bundled styleguide conflicts with an existing guide");
    if(project.styleguide&&!Frame.styleguides.has(project.styleguide.id))Frame.registerStyleguide(project.styleguide);
    archives.set(id,project.archive);blocked.delete(id);
    const doc=project.archive.revisions.find(r=>r.revisionId===project.archive.currentRevisionId).document;
    if(!originalRegister(doc))throw new Error("Project could not be registered");
    persist(id);Frame.agent.selectDesign(id);
    Frame.agent.setViewport(id,project.viewport);Frame.agent.setState(id,project.state);Frame.agent.setValues?.(id,project.values);
    Frame.projectBridge.importFeedback(project.feedback);
    if(project.styleguide) {
      const picker=document.getElementById("styleguide-picker");picker.value=project.styleguide.id;picker.dispatchEvent(new Event("change",{bubbles:true}));
    }
    refresh();return id;
  }
  Frame.validateRevision=doc=>{
    const existing=archives.get(doc.id)?.revisions.find(r=>r.revisionId===doc.revisionId);
    if(existing&&JSON.stringify(existing.document)!==JSON.stringify(doc))throw new Error("A revision ID cannot identify two different documents");
  };
  Frame.revisions={
    list:id=>copy((archives.get(id)?.revisions||[]).map(({revisionId,capturedAt,parentRevisionId,document})=>({revisionId,capturedAt,parentRevisionId,elementCount:document.elements.length}))),
    get:(id,revisionId)=>copy(revision(id,revisionId).document),
    restore:(id,revisionId)=>Frame.agent.updateDocument(copy(revision(id,revisionId).document)),
    compare,
    retry:()=>{for(const id of [...unsaved])persist(id);}
  };
  Frame.project={export:exportProject,validate:validateProject,import:importProject};
  const picker=document.getElementById("revision-picker");
  function refresh() {
    if(!picker)return;
    const id=currentId(),enabled=Frame.documents.has(id);
    picker.disabled=!enabled;
    for(const name of ["restore-revision","compare-revisions","download-project"])document.getElementById(name).disabled=!enabled;
    picker.replaceChildren(...(archives.get(id)?.revisions||[]).map(r=>{const o=document.createElement("option");o.value=r.revisionId;o.textContent=r.revisionId;return o;}));
    picker.value=Frame.designs.get(id)?.revisionId||"";
    notify();
  }
  function download(data,filename,type) {
    const url=URL.createObjectURL(new Blob([data],{type})),link=document.createElement("a");link.href=url;link.download=filename;link.click();setTimeout(()=>URL.revokeObjectURL(url),1000);
  }
  if(picker) {
    document.getElementById("restore-revision").addEventListener("click",()=>Frame.revisions.restore(currentId(),picker.value));
    document.getElementById("compare-revisions").addEventListener("click",()=>{
      const id=currentId(),all=Frame.revisions.list(id),latest=Frame.designs.get(id).revisionId;
      for(const name of ["compare-before","compare-after"]) {
        const select=document.getElementById(name);select.replaceChildren(...all.map(r=>{const o=document.createElement("option");o.value=o.textContent=r.revisionId;return o;}));select.value=name==="compare-before"?picker.value:latest;
      }
      showComparison();document.getElementById("compare-dialog").showModal();
    });
    document.getElementById("compare-before").addEventListener("change",showComparison);
    document.getElementById("compare-after").addEventListener("change",showComparison);
    document.getElementById("compare-overlay").addEventListener("change",()=>document.getElementById("revision-canvases").classList.toggle("overlay",document.getElementById("compare-overlay").checked));
    document.getElementById("close-compare").addEventListener("click",()=>document.getElementById("compare-dialog").close());
    document.getElementById("download-project").addEventListener("click",()=>download(JSON.stringify(exportProject(),null,2),currentId()+".frame.json","application/json"));
    document.getElementById("retry-project-save").addEventListener("click",Frame.revisions.retry);
    const file=document.getElementById("project-file"),confirm=document.getElementById("confirm-project"),status=document.getElementById("project-import-status");
    let draft=null,request=0;
    document.getElementById("import-project").addEventListener("click",()=>{request++;draft=null;file.value="";confirm.disabled=true;status.textContent="";document.getElementById("project-dialog").showModal();});
    file.addEventListener("change",async()=>{
      const current=++request;confirm.disabled=true;draft=null;
      try {if(!file.files[0])return;if(file.files[0].size>32*1024*1024)throw new Error("Project exceeds the 32 MB import limit");
        const input=validateProject(JSON.parse(await file.files[0].text()));if(current!==request)return;
        draft=input;status.textContent="Project "+input.archive.designId+" · "+input.archive.revisions.length+" revisions. Import adds a new design.";confirm.disabled=false;
      }catch(error){if(current===request)status.textContent="Import cancelled: "+error.message;}
    });
    confirm.addEventListener("click",()=>{try{if(draft){importProject(draft);document.getElementById("project-dialog").close();}}catch(error){status.textContent="Import cancelled: "+error.message;}});
    document.getElementById("cancel-project").addEventListener("click",()=>document.getElementById("project-dialog").close());
    document.getElementById("project-dialog").addEventListener("close",()=>{draft=null;request++;});
  }
  function showComparison() {
    const id=currentId(),before=document.getElementById("compare-before").value,after=document.getElementById("compare-after").value,result=compare(id,before,after);
    document.getElementById("compare-summary").textContent=result.added.length+" added · "+result.removed.length+" removed · "+result.changed.length+" changed\n"+
      [...result.added.map(id=>"+ "+id),...result.removed.map(id=>"- "+id),...result.changed.map(item=>item.id+": "+item.fields.join(", ")),...result.settings.map(key=>"Settings: "+key)].join("\n");
    for(const [name,revisionId]of [["before",before],["after",after]]) {
      const doc=revision(id,revisionId).document,canvas=document.getElementById("revision-"+name);canvas.width=doc.width;canvas.height=doc.height;
      canvas.setAttribute("aria-label",name+" revision "+revisionId);FrameDocument.render(canvas.getContext("2d"),doc);
    }
  }
  window.addEventListener("frame:design-updated",e=>{
    if(Frame.documents.has(e.detail.id)){record(Frame.agent.getDocument(e.detail.id));persist(e.detail.id);refresh();}
  });
  window.addEventListener("frame:design-selected",refresh);
  window.addEventListener("beforeunload",event=>{if(unsaved.size){event.preventDefault();event.returnValue="";}});
})();

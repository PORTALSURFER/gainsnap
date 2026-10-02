// Structural checks are visual checks; semantic checks use the actual prototype DOM.
(function(root) {
  function rgb(color) {
    const match=/^#([a-f\d]{6})$/i.exec(color||"");if(!match)return null;
    return [0,2,4].map(i=>parseInt(match[1].slice(i,i+2),16)/255).map(v=>v<=0.04045?v/12.92:((v+0.055)/1.055)**2.4);
  }
  function luminance(color) {const c=rgb(color);return c?c[0]*0.2126+c[1]*0.7152+c[2]*0.0722:null;}
  function contrast(a,b) {const x=luminance(a),y=luminance(b);return x===null||y===null?null:(Math.max(x,y)+0.05)/(Math.min(x,y)+0.05);}
  function check(doc,elements,{measureText=text=>text.length*8,viewport={width:doc.width,height:doc.height}}={}) {
    const issues=[],byId=new Map(elements.map(n=>[n.id,n])),palette={...FrameDocument.tokens.colors,...doc.tokens?.colors};
    const color=value=>palette[value]||value;
    function issue(node,type,message,relatedId) {
      const code=type+(relatedId?":"+relatedId:"");
      if(node.exceptions?.includes(code)||node.exceptions?.includes(type))return;
      issues.push({id:node.id+":"+code,elementId:node.id,type,message,severity:type==="contrast"?"warning":"error",...(relatedId?{relatedId}:{})});
    }
    function ancestor(a,b) {let parent=a.parentId;while(parent){if(parent===b.id)return true;parent=byId.get(parent)?.parentId;}return false;}
    const visible=elements.filter(n=>n.visible!==false);
    for(const node of visible) {
      const b=node.bounds,parent=byId.get(node.parentId),frame=parent?.bounds||{x:0,y:0,...viewport};
      if(b.x<frame.x||b.y<frame.y||b.x+b.width>frame.x+frame.width+0.1||b.y+b.height>frame.y+frame.height+0.1) issue(node,"clipping","Element extends outside its "+(parent?"parent":"viewport"));
      const s=node.style||{},size=s.fontSize||doc.tokens?.typography?.size||14,padding=s.padding??(["button","input"].includes(node.type)?8:0);
      if(node.text) {
        const lines=node.text.split("\n");
        if(lines.some(line=>measureText(line,node)>b.width-padding*2+0.1)||lines.length*(s.lineHeight||size*1.4)>b.height-padding*2+0.1) issue(node,"text-overflow","Text exceeds the element's declared bounds");
        const background=color(s.fill)||color(parent?.style?.fill)||doc.background||palette.background;
        const ratio=contrast(color(s.color)||palette.text,background),minimum=size>=24||size>=18.66&&(s.fontWeight||400)>=700?3:4.5;
        if(ratio!==null&&ratio<minimum) issue(node,"contrast","Text contrast "+ratio.toFixed(2)+":1 is below "+minimum+":1");
      }
    }
    for(let i=0;i<visible.length;i++)for(let j=i+1;j<visible.length;j++) {
      const a=visible[i],b=visible[j];if(ancestor(a,b)||ancestor(b,a)||a.type==="line"||b.type==="line")continue;
      if(a.bounds.x<b.bounds.x+b.bounds.width&&a.bounds.x+a.bounds.width>b.bounds.x&&a.bounds.y<b.bounds.y+b.bounds.height&&a.bounds.y+a.bounds.height>b.bounds.y) {
        if(!a.allowOverlapWith?.includes(b.id)&&!b.allowOverlapWith?.includes(a.id))issue(a,"overlap","Unexpected overlap with "+b.id,b.id);
      }
    }
    const groups=new Map();
    for(const n of visible)if(n.spacingGroup){const items=groups.get(n.spacingGroup)||[];items.push(n);groups.set(n.spacingGroup,items);}
    for(const nodes of groups.values()) {
      if(nodes.length<3)continue;nodes.sort((a,b)=>a.bounds.y-b.bounds.y);
      const gaps=nodes.slice(1).map((n,i)=>n.bounds.y-nodes[i].bounds.y-nodes[i].bounds.height);
      if(Math.max(...gaps)-Math.min(...gaps)>2)issue(nodes[0],"spacing","Repeated group has inconsistent vertical spacing");
    }
    return {kind:"visual",designId:doc.id,revisionId:doc.revisionId,viewport,issues};
  }
  const api={contrast,check};
  if(typeof module!=="undefined") {globalThis.FrameDocument=require("./design-document.js");module.exports=api;}
  else root.FrameReview=api;
})(typeof window==="undefined"?globalThis:window);

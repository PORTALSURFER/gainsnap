// GainSnap concept, authored against Frame 844f3b4 / Technical Futurist.
// Meter values and matching states are illustrative; this preview has no audio engine.
(() => {
  const C = Frame.components;
  const elements = [];
  const add = (type,id,x,y,w,h,options={}) => elements.push(C[type](id,{x,y,width:w,height:h},options));
  const text = (id,value,x,y,w,size=11,color='muted',parentId) => add('text',id,x,y,w,size*1.5,{text:value,parentId,style:{fontSize:size,color}});
  const line = (id,x,y,w,h=0,color='border') => add('line',id,x,y,w,h,{style:{stroke:color}});
  const button = (id,value,x,y,w,action,style={}) => add('button',id,x,y,w,28,{text:value,label:value,action:{type:'set-state',state:action},style:{fontSize:10,padding:6,fill:'surface',stroke:'border',color:'text',radius:3,cutCorner:6,...style}});
  // Structural backing sits behind the foreground control faces.
  add('container','control-backing',106,110,128,224,{decorative:true,allowOverlapWith:['peak-display','rms-display','peak-value','rms-value','gain-label','gain-dial','gain-cap','gain','gain-unit','peak','rms'],points:[[0,0],[122,0],[128,6],[128,214],[118,224],[0,224]],style:{fillTop:'#343D35',fillBottom:'#232923',stroke:'none'}});
  add('container','header-background-band',0,40,240,12,{decorative:true,points:[[0,0],[197,0],[209,12],[0,12]],style:{fill:'#2D332E',stroke:'none'}});
  text('version','v0.1.3',194,3,42,6,'#4C5250');
  elements.find(e=>e.id==='version').style.textAlign='right';
  elements.find(e=>e.id==='version').exceptions=['contrast']; // Deliberately subtle version metadata per the styleguide.
  text('brand','PORTALSURFER',12,20,112,12,'text');
  text('brand-separator','/',130,20,12,12,'muted');
  text('brand-device','GAINSNAP',154,20,74,12,'accent');
  line('header-rule',12,56,216);
  text('meter-title','OUTPUT',12,65,52,9);
  text('meter-unit','dBFS',66,65,30,9);
  text('mode-title','MODE',112,65,116,9);
  line('vertical-divider',102,65,0,322);
  // Quiet tertiary detail: a short trace, square terminals and local diagonal hatching.
  add('line','detail-trace',112,88,83,15,{decorative:true,points:[[0,15],[28,15],[43,0],[83,0]],style:{stroke:'#363D37'}});
  add('container','detail-terminal',195,86,4,4,{decorative:true,style:{fill:'#363D37',stroke:'none'}});
  [0,1,2].forEach(i=>add('line','detail-hatch-'+i,207+i*6,94,5,7,{decorative:true,points:[[0,7],[5,0]],style:{stroke:'#3B423C'}}));
  add('container','meter',40,80,30,242,{style:{fill:'#1F2422',stroke:'border'}});
  add('container','peak-level',4,34,9,204,{parentId:'meter',style:{fill:'accent',stroke:'none'}});
  add('container','rms-level',17,78,9,160,{parentId:'meter',style:{fill:'#C8CDCA',stroke:'none'}});
  ['0','−6','−12','−18','−24','−30','−∞'].forEach((value,i)=>{
    text('scale-'+i,value,8,79+i*39,19,9);
    line('tick-'+i,34,84+i*39,4);
  });
  line('target-line',38,162,42,0,'accent');
  text('target-marker','▶',28,157,10,10,'accent');
  text('gain-marker','◀',72,117,12,12,'text');
  add('container','peak-display',108,118,124,58,{style:{fill:'#1F2422',stroke:'border',radius:2}});
  add('container','rms-display',108,178,124,58,{style:{fill:'#1F2422',stroke:'border',radius:2}});
  text('peak-value','−6.2',112,140,76,21,'text');
  text('rms-value','−12.0',112,200,76,21,'accent');
  line('gain-rule',112,240,116);
  text('gain-label','GAIN',112,251,116,9);
  add('container','gain-dial',112,274,48,48,{style:{radius:24,fill:'none',stroke:'border'}});
  add('container','gain-cap',7,7,34,34,{parentId:'gain-dial',style:{radius:17,fill:'surface',stroke:'#A2ABA4'}});
  add('line','gain-needle',24,5,0,14,{parentId:'gain-dial',style:{stroke:'accent',lineWidth:1}});
  add('input','gain',174,287,54,20,{label:'Manual gain in dB',value:'+3.8',style:{fontSize:13,padding:2,textAlign:'center',verticalAlign:'middle',fill:'surface',stroke:'border'}});
  text('gain-unit','dB',170,316,58,9);
  text('target-label','TARGET',12,337,86,9);
  add('input','target',25,360,60,22,{label:'Target level in dBFS',value:'−12.0',style:{fontSize:15,padding:2,textAlign:'center',verticalAlign:'middle',fill:'surface',stroke:'accent',color:'accent'}});
  button('match','MATCH',112,355,72,'listening',{fill:'#1F2422',stroke:'accent',color:'accent',fontWeight:700});
  button('restart','↻',192,355,36,'listening');
  elements.find(e=>e.id==='restart').label='Restart matching measurement';
  line('footer-rule',12,397,216);
  ['listening','adjusting','matched'].forEach((stage,index)=>add('container','activity-'+stage,12+index*74,406,68,5,{label:stage,style:{fill:'border',stroke:'none'}}));
  const doc={version:1,id:'gainsnap-futurist',name:'GainSnap · Technical Futurist',revisionId:'gainsnap-r24',width:240,height:424,
    background:'#272B28',tokens:{colors:{background:'#272B28',surface:'#303732',text:'#D5D8D6',muted:'#A2ABA4',border:'#49534C',accent:'#E96B50',focus:'#8CDDD0'},typography:{family:'monospace',size:11}},
    states:{default:{},peak:{peak:{style:{fill:'#1F2422',stroke:'#1F2422',color:'accent'}},rms:{style:{stroke:'#1F2422',color:'text'}}},
      listening:{'activity-listening':{style:{fill:'accent'}},match:{text:'ON',style:{fill:'#1F2422',color:'accent'}}},
      adjusting:{'activity-adjusting':{style:{fill:'accent'}},match:{text:'ON',style:{fill:'#1F2422',color:'accent'}}},
      matched:{'activity-matched':{style:{fill:'accent'}},match:{text:'ON',style:{fill:'#1F2422',color:'accent'}}},
      silence:{'activity-listening':{style:{fill:'accent'}},'peak-value':{text:'−∞'},'rms-value':{text:'−∞'}},
      held:{match:{text:'MATCH'}}},elements};
  doc.states.peak['peak-value']={style:{color:'accent'}};
  doc.states.peak['rms-value']={style:{color:'text'}};
  // State controls illustrate listening, settled, silence and held presentations.
  doc.states.silence['peak-level']={visible:false};
  doc.states.silence['rms-level']={visible:false};
  for(const kind of ['peak','rms']) for(const suffix of ['value']) {
    const node=elements.find(e=>e.id===kind+'-'+suffix);
    node.parentId=kind+'-display'; node.bounds.x-=108; node.bounds.y-=kind==='peak'?118:178;
  }
  add('container','chassis-top-inset',16,0,160,9,{points:[[0,0],[160,0],[148,9],[4,9]],style:{fillTop:'#59615A',fillBottom:'#303831',stroke:'none'}});
  add('container','chassis-bottom-inset',26,420,188,4,{points:[[0,4],[4,0],[184,0],[188,4]],style:{fillTop:'#303831',fillBottom:'#59615A',stroke:'none'}});
  add('line','chassis-top',16,0,160,9,{points:[[0,0],[4,9],[148,9],[160,0]],style:{stroke:'border'}});
  add('line','chassis-bottom',26,420,188,4,{points:[[0,4],[4,0],[184,0],[188,4]],style:{stroke:'border'}});
  add('button','peak',4,4,116,16,{parentId:'peak-display',text:'PEAK',label:'Use Peak matching',action:{type:'set-state',state:'peak'},style:{fontSize:9,padding:1,fill:'#1F2422',stroke:'#1F2422',color:'muted'}});
  add('button','rms',4,4,116,16,{parentId:'rms-display',text:'RMS',label:'Use RMS matching',action:{type:'set-state',state:'default'},style:{fontSize:9,padding:1,fill:'#1F2422',stroke:'#1F2422',color:'accent'}});
  // Dark chamfered side recesses sit inside the continuous rectangular window frame.
  add('container','chassis-left-cut',1,208,3,100,{decorative:true,points:[[0,0],[3,6],[3,94],[0,100]],style:{fill:'#1B211E',stroke:'none'}});
  add('container','chassis-left-lip',4,214,2,88,{decorative:true,points:[[0,0],[2,4],[2,84],[0,88]],style:{fillTop:'#4A544C',fillBottom:'#303831',stroke:'none'}});
  add('container','chassis-right-cut',236,82,3,100,{decorative:true,points:[[3,0],[0,6],[0,94],[3,100]],style:{fill:'#1B211E',stroke:'none'}});
  add('container','chassis-right-lip',234,88,2,88,{decorative:true,points:[[2,0],[0,4],[0,84],[2,88]],style:{fillTop:'#4A544C',fillBottom:'#303831',stroke:'none'}});
  add('line','chassis-outline',0.5,0.5,239,423,{decorative:true,points:[[0,0],[239,0],[239,423],[0,423],[0,0]],style:{stroke:'border'}});
  Frame.registerDocument(doc);
})();

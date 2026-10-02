// Keep Frame's complete toolset available without an oversized review surface.
(() => {
  const button=document.createElement('button');
  button.id='device-tools-toggle';button.type='button';button.textContent='More tools';button.setAttribute('aria-expanded','false');
  button.addEventListener('click',()=>{const expanded=document.body.classList.toggle('expanded-tools');button.textContent=expanded?'Fewer tools':'More tools';button.setAttribute('aria-expanded',String(expanded));});
  document.querySelector('.tools').append(button);
  const viewport=document.getElementById('viewport-picker');
  viewport.options[0].textContent='Design size · 240 × 424';
  const id=document.getElementById('design-picker').value;
  const doc=Frame.agent.getDocument(id);
  Frame.agent.setViewport(id,{width:doc.width,height:doc.height});
})();

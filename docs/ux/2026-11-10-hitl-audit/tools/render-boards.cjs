// Local preview of the HITL boards (scratch only): node render.js
const {chromium} = require('/opt/node-tools/node_modules/playwright');
const fs=require('fs'),path=require('path');
const dir=path.join(__dirname,'..','boards'), out=process.argv[2]||'/tmp/hitl-boards';
fs.mkdirSync(out,{recursive:true});
const cj=JSON.parse(fs.readFileSync(dir+'/canvas.json'));
(async()=>{
 const b=await chromium.launch();
 for(const f of Object.keys(cj.boards)){
  if(!fs.existsSync(path.join(dir,f))) continue;
  const bd=cj.boards[f];
  const ctx=await b.newContext({viewport:{width:bd.w,height:bd.h},deviceScaleFactor:1});
  const p=await ctx.newPage(); const errs=[];
  p.on('pageerror',e=>errs.push(e.message));
  await p.route('**/support.js',r=>r.fulfill({status:200,contentType:'application/javascript',body:'window.DCLogic=class{};'}));
  await p.goto('file://'+path.join(dir,f)); await p.waitForTimeout(1200);
  const d=await p.evaluate(()=>{
    const over=[]; for(const el of document.querySelectorAll('.app *')){ if(el.scrollWidth>el.clientWidth+2 && getComputedStyle(el).overflow==='visible' && el.children.length===0 && el.clientWidth>0) over.push((el.textContent||'').trim().slice(0,40)); }
    return {sw:document.documentElement.scrollWidth,sh:document.documentElement.scrollHeight,font:getComputedStyle(document.querySelector('.app')).fontFamily.slice(0,20),geist:document.fonts.check('13px Geist'),over:over.slice(0,8)};
  });
  await p.screenshot({path:path.join(out,f.replace('.dc.html','.png')),fullPage:true});
  console.log(f, `frame=${bd.w}x${bd.h}`, `scroll=${d.sw}x${d.sh}`, 'geist='+d.geist, 'errs='+errs.length, d.over.length?('overflow:'+JSON.stringify(d.over)):'');
  await ctx.close();
 }
 await b.close();
})();

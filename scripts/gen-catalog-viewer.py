#!/usr/bin/env python3
"""Render the hardware catalog as a single self-contained HTML page (Projects -> Modules -> Chips),
Voltkid-style: tabs, search, kind filter, cards that expand. A quick way to SEE the catalog before
the console/dashboard browse views exist. Embeds catalog.json inline so the file opens offline.

Usage: scripts/gen-catalog-viewer.py [catalog.json] [out.html]
"""
import json, sys, html

CAT = sys.argv[1] if len(sys.argv) > 1 else "crates/weftos-cog-market/catalog/catalog.json"
OUT = sys.argv[2] if len(sys.argv) > 2 else "/tmp/weftos-catalog.html"
data = json.load(open(CAT))

PAGE = """<!doctype html><html lang=en><head><meta charset=utf-8>
<meta name=viewport content="width=device-width,initial-scale=1">
<title>WeftOS Hardware Catalog</title>
<style>
:root{--bg:#f6efe9;--card:#fff;--ink:#23262b;--grey:#7c818a;--line:#e7ddd3;--wl:#6c9ce8;--accent:#e08a3a;--green:#3f9d63}
@media(prefers-color-scheme:dark){:root:not([data-theme=light]){--bg:#1b1d20;--card:#24272c;--ink:#e6e8ea;--grey:#9aa0a8;--line:#333]}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--ink);font:15px/1.5 -apple-system,BlinkMacSystemFont,Segoe UI,Roboto,sans-serif}
header{padding:20px 24px 8px}h1{margin:0;font-size:26px}.sub{color:var(--grey);margin:2px 0 0}
.bar{position:sticky;top:0;background:var(--bg);padding:10px 24px;border-bottom:1px solid var(--line);z-index:5;display:flex;gap:10px;flex-wrap:wrap;align-items:center}
.tab{padding:7px 14px;border:1px solid var(--line);border-radius:20px;background:var(--card);cursor:pointer;font-weight:600}
.tab.on{background:var(--ink);color:var(--bg);border-color:var(--ink)}
.chip{padding:5px 11px;border:1px solid var(--line);border-radius:16px;background:var(--card);cursor:pointer;font-size:13px}
.chip.on{background:var(--wl);color:#fff;border-color:var(--wl)}
input{padding:8px 12px;border:1px solid var(--line);border-radius:18px;background:var(--card);color:var(--ink);min-width:220px;flex:1}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(320px,1fr));gap:14px;padding:16px 24px 60px}
.c{background:var(--card);border:1px solid var(--line);border-radius:12px;padding:14px 16px;cursor:pointer}
.c h3{margin:0 0 2px;font-size:17px}.v{color:var(--grey);font-size:13px}
.c p{margin:8px 0 0;color:var(--ink)}
.row{display:flex;gap:8px;flex-wrap:wrap;margin-top:10px;align-items:center}
.pill{font-size:12px;padding:3px 9px;border-radius:12px;background:var(--bg);border:1px solid var(--line);color:var(--grey)}
.buy{color:#fff;background:var(--green);border:none}.kindb{color:#fff;background:var(--wl);border:none;text-transform:capitalize}
a{color:var(--wl)}.det{display:none;margin-top:10px;border-top:1px solid var(--line);padding-top:10px;font-size:14px}
.c.open .det{display:block}.k{color:var(--grey)}.note{background:#fff6e9;border-left:3px solid var(--accent);padding:8px 10px;border-radius:6px;margin:6px 0;font-size:13px}
@media(prefers-color-scheme:dark){:root:not([data-theme=light]) .note{background:#2a2620}}
.count{color:var(--grey);font-size:13px;margin-left:auto}
</style></head><body>
<header><h1>WeftOS Hardware Catalog</h1><p class=sub>Projects &rarr; Modules &rarr; Chips &middot; every sensor we've explored &middot; <span id=stat></span></p></header>
<div class=bar>
 <span class=tab data-t=projects>Projects</span><span class="tab on" data-t=modules>Modules</span><span class=tab data-t=chips>Chips</span>
 <input id=q placeholder="search name, vendor, spec, what it senses…">
 <span class=count id=cnt></span>
</div>
<div id=chips class=bar style="border:none;padding-top:0"></div>
<div class=grid id=grid></div>
<script>const CATALOG=__DATA__;</script>
<script>
const G=document.getElementById('grid'),Q=document.getElementById('q'),CF=document.getElementById('chips');
let tab='modules',kind='all',q='';
const KINDS=['all','board','sensor','display','actuator'];
function esc(s){return (s==null?'':''+s).replace(/[&<>]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;'}[c]))}
function price(x){const b=(x.buy||[])[0];return b&&b.price?b.price:''}
function specRows(sp){return Object.entries(sp||{}).map(([k,v])=>`<div><span class=k>${esc(k)}:</span> ${esc(v)}</div>`).join('')}
function card(x,type){
 const ds=x.datasheet?`<a href="${esc(x.datasheet)}" target=_blank onclick="event.stopPropagation()">datasheet ↗</a>`:'';
 const b=(x.buy||[])[0];const buy=b?`<a class="pill buy" href="${esc(b.url)}" target=_blank onclick="event.stopPropagation()">${esc(b.price||'Mouser')} ↗</a>`:'';
 const vendor=x.vendor||x.manufacturer||'';
 let pills='';
 if(type==='modules'){pills=`<span class="pill kindb">${esc(x.kind||'sensor')}</span>`+(x.chips||[]).map(c=>`<span class=pill>◉ ${esc(c)}</span>`).join('')}
 if(type==='chips'){pills=(x.tags||[]).slice(0,5).map(t=>`<span class=pill>${esc(t)}</span>`).join('')}
 if(type==='projects'){pills=`<span class="pill kindb">${esc(x.category||'')}</span><span class=pill>${esc(x.difficulty||'')}</span>`+(x.modules||[]).map(m=>`<span class=pill>▸ ${esc(m)}</span>`).join('')}
 const notes=(x.notes||[]).map(n=>`<div class=note>${esc(n)}</div>`).join('');
 const gf=(x.good_for||[]).length?`<div style=margin-top:8px><b>Good for:</b> ${x.good_for.map(esc).join(' · ')}</div>`:'';
 const nf=(x.not_for||[]).length?`<div><b>Not for:</b> ${x.not_for.map(esc).join(' · ')}</div>`:'';
 const seen=(x.seen_in||[]).length?`<div class=v style=margin-top:8px>seen in: ${x.seen_in.map(esc).join(', ')}</div>`:'';
 return `<div class=c onclick="this.classList.toggle('open')">
  <h3>${esc(x.name)}</h3><div class=v>${esc(vendor)}</div>
  <p>${esc(x.summary||x.role||'')}</p>
  <div class=row>${pills}</div>
  <div class=row>${buy} ${ds}</div>
  <div class=det>${gf}${nf}${notes}<div style=margin-top:8px>${specRows(x.spec)}</div>${seen}</div>
 </div>`;
}
function render(){
 let items=CATALOG[tab]||[];
 const ql=q.toLowerCase();
 if(tab==='modules'&&kind!=='all')items=items.filter(x=>(x.kind||'sensor')===kind);
 if(ql)items=items.filter(x=>JSON.stringify(x).toLowerCase().includes(ql));
 G.innerHTML=items.map(x=>card(x,tab)).join('')||'<p style=color:var(--grey)>nothing matches</p>';
 document.getElementById('cnt').textContent=items.length+' shown';
 CF.innerHTML = tab==='modules'?KINDS.map(k=>`<span class="chip ${k===kind?'on':''}" data-k="${k}">${k}</span>`).join(''):'';
 CF.querySelectorAll('.chip').forEach(c=>c.onclick=()=>{kind=c.dataset.k;render()});
}
document.querySelectorAll('.tab').forEach(t=>t.onclick=()=>{document.querySelectorAll('.tab').forEach(e=>e.classList.remove('on'));t.classList.add('on');tab=t.dataset.t;kind='all';render()});
Q.oninput=()=>{q=Q.value;render()};
document.getElementById('stat').textContent=`${CATALOG.projects.length} projects · ${CATALOG.modules.length} modules · ${CATALOG.chips.length} chips`;
render();
</script></body></html>"""

html_out = PAGE.replace("__DATA__", json.dumps(data))
open(OUT, "w").write(html_out)
print(f"wrote {OUT} ({len(html_out)//1024} KB)  — {len(data['projects'])} projects, {len(data['modules'])} modules, {len(data['chips'])} chips")

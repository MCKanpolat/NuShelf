const timestamp=value=>value?new Date(value).toLocaleString():'—';
export async function mountIntegrity(content,{api,esc,date,toast}) {
  const request=(path='',options={})=>api('/api/integrity'+path,options,true);
  async function reload(){
    const state=await request(),p=state.policy;
    content.innerHTML='<div class="section-heading"><h2>Storage integrity</h2><div class="inline-actions"><button class="button" data-scan="refresh">Refresh</button><button class="button primary" data-scan="start">Run scan now</button></div></div><p>Verify the size and hash of committed package, cache and symbol files in this feed. Scans report issues without changing or deleting files.</p><div class="panel"><h3>Scheduled scans</h3><form data-scan-form><label><input type="checkbox" name="enabled" '+(p.enabled?'checked':'')+'> Enable periodic scans</label><div class="field"><label for="integrity-hours">Interval (hours)</label><input id="integrity-hours" type="number" name="hours" min="1" max="8760" value="'+p.intervalHours+'" required></div><p>Next scan: '+(p.enabled?esc(timestamp(p.nextScan)):'Automatic scans disabled')+'</p><button class="button" type="submit">Save scan schedule</button><div class="form-error" role="alert"></div></form></div><div class="panel"><h3>Scan history</h3>'+ (state.runs.map(r=>{
      const c=r.counts||{},total=Object.values(c).reduce((a,b)=>a+b,0),issues=(c.missing||0)+(c.mismatch||0)+(c.unavailable||0);
      return '<div class="settings-row"><span><strong>'+esc(timestamp(r.createdAt))+'</strong> · '+esc(r.status)+'<br>'+(total-(c.pending||0))+' / '+total+' checked · '+issues+' issues · '+(c.obsolete||0)+' obsolete references<br><small>'+esc(r.id)+'</small></span><button class="button small" data-scan="details" data-id="'+esc(r.id)+'">View issues</button></div>';
    }).join('')||'<p>No scans yet.</p>')+'</div><div class="form-error" role="alert" data-scan-error></div><div class="panel" data-scan-details hidden></div>';
  }
  async function details(id,skip=0){
    const result=await request('/runs/'+id+'?take=100&skip='+skip);
    const panel=content.querySelector('[data-scan-details]');panel.hidden=false;
    panel.innerHTML='<h3>Scan issues</h3><p>Missing: object absent. Mismatch: wrong bytes or size. Unavailable: storage error or timeout.</p>'+(result.items.length?'<div class="table-wrap"><table class="version-table"><thead><tr><th>Package</th><th>Version</th><th>Object</th><th>Result</th></tr></thead><tbody>'+result.items.map(i=>'<tr><td>'+esc(i.packageId)+'</td><td>'+esc(i.version)+'</td><td>'+esc(i.kind)+'</td><td>'+esc(i.status)+'<br><small>Expected '+i.expectedSize+' bytes'+(i.actualSize!==null?' · Read '+i.actualSize:'')+'</small></td></tr>').join('')+'</tbody></table></div>':'<p>No issues recorded on this page. Pending objects have not been checked yet.</p>')+'<div class="inline-actions">'+(skip?'<button class="button small" data-scan="details" data-id="'+esc(id)+'" data-skip="'+(skip-100)+'">Previous issues</button>':'')+(result.items.length===100?'<button class="button small" data-scan="details" data-id="'+esc(id)+'" data-skip="'+(skip+100)+'">Next issues</button>':'')+'</div>';
  }
  content.onsubmit=async event=>{
    if(!event.target.matches('[data-scan-form]'))return;
    event.preventDefault();event.stopPropagation();const form=event.target,button=form.querySelector('button');button.disabled=true;
    try {const data=new FormData(form);await request('',{method:'PUT',body:{enabled:data.has('enabled'),intervalHours:Number(data.get('hours'))}});await reload();toast('Integrity schedule saved.');}
    catch(e){form.querySelector('.form-error').textContent=e.message;}finally{button.disabled=false;}
  };
  content.onclick=async event=>{
    const button=event.target.closest('[data-scan]');if(!button)return;event.stopPropagation();button.disabled=true;
    try {if(button.dataset.scan==='details'){await details(button.dataset.id,Number(button.dataset.skip||0));}else{if(button.dataset.scan==='start'){await request('/runs',{method:'POST'});toast('Integrity scan queued.');}await reload();}}
    catch(e){content.querySelector('[data-scan-error]').textContent=e.message;}finally{button.disabled=false;}
  };
  await reload();
}

export async function mountUsage(content,{api,esc,date,toast}) {
  let revision;
  const mib=1048576,number=n=>new Intl.NumberFormat('en-US',{maximumFractionDigits:2}).format(n);
  const bytes=n=>number(n/mib)+' MiB';
  async function reload(){
    const result=await api('/api/usage',{},true),u=result.usage;revision=u.revision;
    content.innerHTML='<div class="section-heading"><h2>Feed usage & quota</h2><button class="button" data-usage-refresh>Refresh usage</button></div><p>Limits apply to committed locally published packages and extracted symbols in the current feed. Zero means unlimited. Upstream cache has its own cleanup policy.</p><div class="mini-stats">'+[[number(u.packages),'Packages'],[number(u.versions),'Versions'],[bytes(u.totalBytes),'Published storage'],[bytes(result.cache.bytes),'Upstream cache']].map(([n,label])=>'<div class="mini-stat"><strong>'+esc(n)+'</strong><span>'+label+'</span></div>').join('')+'</div>'+(u.overQuota?'<p class="error-message">Current usage exceeds a configured limit. Existing packages remain available; operations which exceed a limit are rejected.</p>':'')+'<div class="panel"><h3>Publication limits</h3><form data-usage-form><div class="form-grid"><div class="field"><label for="usage-bytes">Published storage limit (MiB)</label><input id="usage-bytes" name="storage" type="number" min="0" step="any" value="'+(u.maxBytes/mib)+'" required><small>Includes '+esc(bytes(u.packageBytes))+' of archives and '+esc(bytes(u.symbolBytes))+' of symbols.</small></div><div class="field"><label for="usage-packages">Maximum package IDs</label><input id="usage-packages" name="packages" type="number" min="0" step="1" value="'+u.maxPackages+'" required></div><div class="field"><label for="usage-versions">Maximum package versions</label><input id="usage-versions" name="versions" type="number" min="0" step="1" value="'+u.maxVersions+'" required></div></div><button class="button primary" type="submit">Save publication limits</button><div class="form-error" role="alert"></div></form></div><div class="panel table-wrap"><h3>Daily usage history</h3><p>Updated every five minutes. The most recent sample for each UTC day is retained.</p><table class="version-table"><thead><tr><th>Day</th><th>Packages</th><th>Versions</th><th>Published storage</th><th>Sampled</th></tr></thead><tbody>'+result.history.map(h=>'<tr><td>'+esc(h.day)+'</td><td>'+number(h.packages)+'</td><td>'+number(h.versions)+'</td><td>'+esc(bytes(h.packageBytes+h.symbolBytes))+'</td><td>'+esc(timestamp(h.sampledAt))+'</td></tr>').join('')+'</tbody></table></div><div class="form-error" role="alert" data-usage-error></div>';
  }
  content.onsubmit=async event=>{
    if(!event.target.matches('[data-usage-form]'))return;
    event.preventDefault();event.stopPropagation();const form=event.target,button=form.querySelector('button');button.disabled=true;
    try {const data=new FormData(form);const body={maxBytes:Math.round(Number(data.get('storage'))*mib),maxPackages:Number(data.get('packages')),maxVersions:Number(data.get('versions')),revision};if(![body.maxBytes,body.maxPackages,body.maxVersions].every(v=>Number.isSafeInteger(v)&&v>=0))throw new Error('Enter nonnegative limits within the supported integer range.');await api('/api/usage',{method:'PUT',body},true);await reload();toast('Publication limits saved.');}
    catch(e){form.querySelector('.form-error').textContent=e.message;}finally{button.disabled=false;}
  };
  content.onclick=async event=>{
    const button=event.target.closest('[data-usage-refresh]');if(!button)return;event.stopPropagation();button.disabled=true;
    try{await reload();}catch(e){content.querySelector('[data-usage-error]').textContent=e.message;}finally{button.disabled=false;}
  };
  await reload();
}

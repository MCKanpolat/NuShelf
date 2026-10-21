export function mountDiagnostics(root,{api,esc}) {
  root.innerHTML=`<h2>Restore diagnostics</h2><p>Explain package routing and access for your current key and feed. Live checks read upstream metadata without downloading packages.</p><form id="restore-diagnostics"><div class="form-grid"><div class="field"><label for="diagnostic-id">Package ID</label><input id="diagnostic-id" name="id" required maxlength="100"></div><div class="field"><label for="diagnostic-version">Exact version</label><input id="diagnostic-version" name="version" required maxlength="128"></div></div><label><input type="checkbox" name="probe"> Check live upstream metadata</label><p><button class="button primary" type="submit">Explain resolution</button></p></form><div id="diagnostic-result" aria-live="polite"></div>`;
  root.querySelector('form').addEventListener('submit',async e=>{
    e.preventDefault();const button=e.target.querySelector('button'),target=root.querySelector('#diagnostic-result');button.disabled=true;target.textContent='Reading resolver policy…';
    try {
      const data=new FormData(e.target),q=new URLSearchParams({id:data.get('id'),version:data.get('version'),probe:String(data.has('probe'))});
      const report=await api('/api/diagnostics/restore?'+q,{},true);
      if(!root.isConnected)return;
      target.innerHTML=`<div class="panel"><h3>${esc(report.code)}</h3><p>${esc(report.limitations)}</p><ol>${report.steps.map(s=>`<li><strong>${esc(s.source||s.stage)}: ${esc(s.code||s.result?.code)}</strong>${s.message?`<p>${esc(s.message)}</p>`:''}${s.result?`<pre>${esc(JSON.stringify(s.result,null,2))}</pre>`:''}${s.patterns?`<p>Include: ${esc(s.patterns.join(', '))} · Exclude: ${esc(s.excludes.join(', '))} · Priority: ${esc(s.priority)}${s.pinned?' · Origin pinned':''}</p>`:''}</li>`).join('')}</ol><h3>NuGet.Config for this feed</h3><p>This configuration routes all packages through this feed. Configure private-feed credentials separately. Existing global-cache entries may bypass source mapping.</p><pre>${esc(report.nugetConfig)}</pre></div>`;
    }catch(error){target.textContent=error.message;}finally{button.disabled=false;}
  });
}

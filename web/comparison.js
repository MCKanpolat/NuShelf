export function mountComparison(root,{api,esc,date}) {
  root.innerHTML=`<h2>Compare package versions</h2><p>Compare locally published or already cached archives. APICompat checks API compatibility in both directions; the report also includes content, dependency, framework and runtime changes.</p><form><div class="field"><label for="compare-id">Package ID</label><input id="compare-id" name="id" required maxlength="100"></div><div class="form-grid"><div class="field"><label for="compare-baseline">Baseline version</label><input id="compare-baseline" name="baselineVersion" required maxlength="128"></div><div class="field"><label for="compare-target">Newer version</label><input id="compare-target" name="targetVersion" required maxlength="128"></div></div><button class="button primary" type="submit">Start comparison</button> <button class="button" id="comparison-history" type="button">Load history</button></form><div id="comparison-status" role="status"></div><div id="comparison-results"></div>`;
  const status=root.querySelector('#comparison-status'),results=root.querySelector('#comparison-results'),form=root.querySelector('form');let timer;form.elements.id.value=new URLSearchParams(location.search).get('id')||'';
  const path=id=>'/api/packages/'+encodeURIComponent(id)+'/comparisons';
  const show=async(id,job)=>{
    clearTimeout(timer);
    try {
      const row=await api(path(id)+'/'+encodeURIComponent(job),{},true);
      if(!root.isConnected)return;
      status.textContent=row.status+(row.error?': '+row.error:'');
      if(['queued','running'].includes(row.status)){timer=setTimeout(()=>{if(root.isConnected)show(id,job);},3000);return;}
      if(!row.report)return;
      const r=row.report,section=(title,value)=>`<details><summary>${esc(title)}</summary><pre>${esc(JSON.stringify(value,null,2))}</pre></details>`;
      results.innerHTML=`<div class="panel"><h3>${esc(row.baseline_version)} → ${esc(row.target_version)}</h3>${r.semverReviewRecommended?'<p class="error-message">API findings in the same major version: review the version increment.</p>':''}<p>${esc(r.api.interpretation)}</p>${section('API removals and compatibility findings',r.api.baselineToTarget)}${section('API additions and reverse compatibility findings',r.api.targetToBaseline)}${section('Files',r.files)}${section('Frameworks',r.frameworks)}${section('Runtimes',r.runtimes)}${section('Dependencies and metadata',r.metadata)}${section('Archive sizes',r.compressedBytes)}</div>`;
    }catch(e){if(root.isConnected)status.textContent=e.message;}
  };
  form.addEventListener('submit',async e=>{
    e.preventDefault();clearTimeout(timer);results.replaceChildren();const button=form.querySelector('[type=submit]');button.disabled=true;
    try{const data=Object.fromEntries(new FormData(form)),id=data.id;delete data.id;const job=await api(path(id),{method:'POST',body:data},true);await show(id,job.id);}catch(e){status.textContent=e.message;}finally{button.disabled=false;}
  });
  root.querySelector('#comparison-history').addEventListener('click',async()=>{
    clearTimeout(timer);const id=form.elements.id.value.trim();if(!id){status.textContent='Enter a package ID.';return;}
    try{const rows=await api(path(id),{},true);if(!root.isConnected)return;results.innerHTML=rows.map(r=>`<p><button class="button" data-job="${esc(r.id)}">${esc(r.baselineVersion)} → ${esc(r.targetVersion)}</button> ${esc(r.status)} · ${esc(date(r.createdAt))}</p>`).join('')||'<p>No comparisons yet.</p>';results.querySelectorAll('[data-job]').forEach(b=>b.addEventListener('click',()=>show(id,b.dataset.job)));}catch(e){status.textContent=e.message;}
  });
}

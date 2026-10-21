export function mountTransferRequest(panel, result, {api,esc,toast}) {
  panel.innerHTML='<h3>Transfer ownership</h3><p>The recipient must accept within seven days. All versions and future publishing rights move together.</p><form><div class="field"><label for="transfer-recipient">Recipient profile ID</label><input id="transfer-recipient" name="recipient" required placeholder="Recipient profile UUID"></div><button class="button" type="submit">Request transfer</button><div class="form-error" role="alert"></div><div role="status"></div></form>';
  panel.querySelector('form').onsubmit=async event=>{
    event.preventDefault();event.stopPropagation();
    const form=event.target,button=form.querySelector('button'),error=form.querySelector('.form-error');
    error.textContent='';button.disabled=true;
    try {
      await api('/api/packages/'+encodeURIComponent(result.id)+'/ownership',{method:'POST',body:{targetProfileId:new FormData(form).get('recipient').trim(),expectedOwnerId:result.ownerId}},true);
      form.querySelector('[role=status]').innerHTML='Transfer requested. View its status in <a href="/manage?tab=ownership">Ownership transfers</a>.';
      toast('Ownership transfer requested.');
    } catch(e) {error.textContent=e.message;}
    finally {button.disabled=false;}
  };
}
export async function mountOwnership(content,{api,esc,date,toast,identity}) {
  async function reload() {
    const transfers=await api('/api/ownership/transfers',{},true);
    content.innerHTML='<div class="section-heading"><h2>Ownership transfers</h2></div><p>Transfers apply to every version of a locally published package in this feed. Pending requests expire after seven days.</p>'+(identity.profileId?'<p>Your profile ID: <code>'+esc(identity.profileId)+'</code></p>':'')+'<div class="panel">'+(transfers.map(t=>'<div class="settings-row"><span><a href="/packages/'+encodeURIComponent(t.packageId)+'">'+esc(t.packageId)+'</a><br>'+esc(t.sourceName)+' → '+esc(t.targetName)+'<br>'+esc(t.status)+' · Expires '+esc(date(t.expiresAt))+'</span><span class="inline-actions">'+(t.status==='pending'?((identity.admin||identity.profileId===t.targetProfileId)?'<button class="button small primary" data-transfer="accept" data-id="'+esc(t.id)+'">Accept transfer</button>':'')+'<button class="button small" data-transfer="cancel" data-id="'+esc(t.id)+'">Cancel transfer</button>':'')+'</span></div>').join('')||'<p>No transfers for this profile in this feed.</p>')+'</div><div class="form-error" role="alert"></div>';
  }
  content.onclick=async event=>{
    const button=event.target.closest('[data-transfer]');if(!button)return;
    event.stopPropagation();button.disabled=true;
    try {await api('/api/ownership/transfers/'+button.dataset.id+'/'+button.dataset.transfer,{method:'POST'},true);await reload();toast('Ownership transfer '+(button.dataset.transfer==='accept'?'accepted.':'cancelled.'));}
    catch(e){content.querySelector('.form-error').textContent=e.message;}
    finally{button.disabled=false;}
  };
  await reload();
}

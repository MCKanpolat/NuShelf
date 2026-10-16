// Webhook management owns its form state so failed saves retain the user's edits.
export async function mountWebhooks(content, {api, esc, date, toast}) {
  let hooks = [], deliveries = [], editing = null, draft = null;
  const request = (path, options = {}) => api('/api/webhooks' + path, options, true);
  async function reload() {
    [hooks, deliveries] = await Promise.all([request(''), request('/deliveries')]);
    render();
  }
  function field(name, label, value = '', extra = '') {
    return '<div class="field"><label for="hook-' + name + '">' + label + '</label><input id="hook-' + name + '" name="' + name + '" value="' + esc(value) + '" ' + extra + '></div>';
  }
  function check(name, label, value) {
    return '<label><input name="' + name + '" type="checkbox" ' + (value ? 'checked' : '') + '> ' + label + '</label>';
  }
  function render() {
    const hook = draft || editing || {name:'',url:'',secretEnv:'',events:['package.published'],feedId:null,enabled:true};
    content.innerHTML = `
      <div class="section-heading"><h2>Webhooks</h2><button class="button" data-hook-action="dispatch">Process due deliveries</button></div>
      <div class="form-error" role="alert" data-hook-error></div>
      <section class="panel" data-hook-editor><h2>${editing ? 'Edit webhook' : 'Add a webhook'}</h2>
        <p>Send signed events to your integration service. Signing secrets stay in the deployment environment.</p>
        ${editing ? '<p>Changing the receiver, signing configuration or subscription cancels pending deliveries. Failed and cancelled deliveries can be retried explicitly when they match the current subscription.</p>' : ''}
        <form data-hook-form>
          <div class="form-grid">${field('name','Webhook name',hook.name,'maxlength="80" required')}${field('secretEnv','Signing secret environment variable',hook.secretEnv,'maxlength="128" placeholder="WEBHOOK_RELEASE_SIGNING_KEY" required')}</div>
          ${field('url','Receiver URL',hook.url,'type="url" placeholder="https://ci.example.com/events" required')}
          <div class="form-grid">${field('events','Event types',hook.events.join(', '),'required')}${field('feedId','Feed ID (optional)',hook.feedId || '')}</div>
          <p>Comma-separated event types, or * for all successful management changes. Leave feed ID empty for all feeds.</p>
          <div class="check-list">${check('enabled','Enabled',hook.enabled)}${check('allowHttp','Allow HTTP (development)',hook.allowHttp)}${check('allowPrivateNetwork','Allow private network receiver',hook.allowPrivateNetwork)}</div>
          <div class="inline-actions"><button class="button primary" type="submit">${editing ? 'Save webhook' : 'Create webhook'}</button>${editing ? '<button class="button" type="button" data-hook-action="cancel">Cancel editing</button>' : ''}</div>
          <div class="form-error" role="alert"></div>
        </form>
      </section>
      <section class="panel"><h2>Configured receivers</h2>
        ${hooks.map(h => `<div class="settings-row"><span><strong>${esc(h.name)}</strong><br>${esc(h.url)}<br>${esc(h.events.join(', '))} · ${h.enabled ? 'Enabled' : 'Disabled'}</span><span class="inline-actions"><button class="button small" data-hook-action="edit" data-id="${esc(h.id)}">Edit</button>${h.enabled ? `<button class="button small danger" data-hook-action="disable" data-id="${esc(h.id)}">Disable</button>` : ''}</span></div>`).join('') || '<p>No webhooks configured.</p>'}
      </section>
      <section class="panel table-wrap"><h2>Delivery history</h2><p>Automatic retries stop after six attempts per cycle. Manual retry preserves the delivery ID and total attempts.</p>
        <table class="version-table"><thead><tr><th>Receiver</th><th>Attempts</th><th>Status</th><th>Last error</th><th>Actions</th></tr></thead><tbody>
        ${deliveries.map(d => `<tr><td>${esc(d.name)}<br><small>${esc(d.id)}</small></td><td>${d.attempts} total<br>${d.attemptsInCycle}/6 this cycle<br>${d.retryCount} manual retries</td><td>${esc(d.state)} ${d.status || ''}${d.state === 'pending' ? '<br><small>Next: ' + esc(date(d.nextAttempt)) + '</small>' : ''}</td><td>${esc(d.error || '—')}</td><td><div class="inline-actions">${d.canRetry ? `<button class="button small" data-hook-action="retry" data-id="${esc(d.id)}">Retry delivery</button>` : ''}<button class="button small" data-hook-action="history" data-id="${esc(d.id)}">Attempt history</button></div></td></tr>`).join('') || '<tr><td colspan="5">No deliveries yet.</td></tr>'}
        </tbody></table>
      </section><section class="panel" data-hook-history hidden aria-live="polite"></section>`;
  }
  function readForm(form) {
    const data = new FormData(form);
    return {name:data.get('name'),url:data.get('url'),secretEnv:data.get('secretEnv'),feedId:data.get('feedId').trim() || null,events:data.get('events').split(',').map(e=>e.trim()).filter(Boolean),enabled:data.has('enabled'),allowHttp:data.has('allowHttp'),allowPrivateNetwork:data.has('allowPrivateNetwork')};
  }
  content.onsubmit = async event => {
    if (!event.target.matches('[data-hook-form]')) return;
    event.preventDefault();
    event.stopPropagation();
    const form = event.target, button = form.querySelector('[type=submit]'), error = form.querySelector('.form-error');
    const body = readForm(form);
    if (editing) body.revision = editing.revision;
    button.disabled = true;
    error.textContent = '';
    try {
      await request(editing ? '/' + editing.id : '', {method:editing ? 'PUT' : 'POST',body});
      toast(editing ? 'Webhook saved.' : 'Webhook created.');
      editing = null;
      draft = null;
      await reload();
    } catch (e) {error.textContent = e.message;}
    finally {button.disabled = false;}
  };
  content.onclick = async event => {
    const button = event.target.closest('[data-hook-action]');
    if (!button) return;
    event.stopPropagation();
    const {hookAction:action,id} = button.dataset;
    const error = content.querySelector('[data-hook-error]');
    error.textContent = '';
    button.disabled = true;
    try {
      if (action === 'edit') {
        draft = null;
        editing = hooks.find(h=>h.id === id);
        render();
        content.querySelector('[data-hook-editor]').scrollIntoView({block:'start'});
        content.querySelector('[name=name]').focus();
      } else if (action === 'cancel') {editing = null; draft = null; render();}
      else if (action === 'history') {
        const rows = await request('/deliveries/' + id + '/attempts');
        const panel = content.querySelector('[data-hook-history]');
        panel.hidden = false;
        panel.innerHTML = '<h2>Attempt history</h2><p>Delivery ' + esc(id) + '</p>' +
          (rows.length ? rows.map(a=>'<div class="settings-row"><span><strong>Attempt ' + a.attempt + '</strong> · Cycle ' + (a.cycle+1) + '<br>' + esc(a.receiverUrl) + ' · Revision ' + a.webhookRevision + '<br>' + esc(date(a.startedAt)) + '</span><span>' + (a.status || 'Connection failed') + '<br>' + esc(a.error || 'Accepted') + '</span></div>').join('') : '<p>No recorded attempts. Detailed history is available for attempts made after this feature was installed.</p>');
        panel.scrollIntoView({block:'nearest'});
      } else {
        if (action === 'disable') {await request('/' + id,{method:'DELETE'});toast('Webhook disabled.');}
        if (action === 'dispatch') {await request('/dispatch',{method:'POST'});toast('Due deliveries processed.');}
        if (action === 'retry') {await request('/deliveries/' + id + '/retry',{method:'POST'});toast('Delivery queued for retry.');}
        draft = readForm(content.querySelector('[data-hook-form]'));
        await reload();
      }
    } catch (e) {error.textContent = e.message;}
    finally {button.disabled = false;}
  };
  await reload();
}

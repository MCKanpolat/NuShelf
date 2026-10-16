/** Live upstream discovery and durable mirror controls. Dependencies are injected
 * so registry navigation and credentials remain owned by the application. */
export function mountFederation(container, {api, packageRows, esc, query, prerelease, authenticated}) {
  container.innerHTML = '<div class="section-heading"><h2>Search upstream sources</h2><button class="button" type="button">Search upstreams</button></div><p>Discover packages that have not been cached yet. Each source has its own results and pages.</p><div class="upstream-search-pages" aria-live="polite"></div>';
  const results = container.querySelector('.upstream-search-pages');
  const button = container.querySelector('button');
  async function load(sourceId, skip = 0, target = results) {
    const params = new URLSearchParams({q: query, prerelease: String(prerelease), semVerLevel: '2.0.0', take: '20', skip: String(skip)});
    if (sourceId) params.set('sourceId', sourceId);
    target.textContent = 'Searching upstream sources…';
    try {
      const response = await api(`/api/upstreams/search?${params}`, {}, authenticated);
      if (!container.isConnected) return;
      target.innerHTML = response.sources.map(source => `<section class="panel" data-source-page="${esc(source.sourceId)}"><h3>${esc(source.name)}</h3>${source.error ? `<p class="error-message" role="alert">${esc(source.error)}</p><button class="button" type="button" data-page-retry="${esc(source.sourceId)}" data-skip="${skip}">Retry source</button>` : `<p>${source.data.length} visible results on this page · ${source.upstreamTotalHits} results reported by upstream before routing filters.</p>${source.data.length ? packageRows(source.data) : '<p>No visible packages on this page.</p>'}${source.nextSkip !== null ? `<button class="button" type="button" data-page-next="${esc(source.sourceId)}" data-skip="${source.nextSkip}">Next source page →</button>` : ''}`}</section>`).join('') || '<p>No visible upstream sources configured.</p>';
    } catch (error) { target.innerHTML = `<p class="error-message" role="alert">${esc(error.message)}</p>`; }
  }
  button.addEventListener('click', async () => { button.disabled = true; try { await load(); } finally { button.disabled = false; } });
  results.addEventListener('click', event => {
    const next = event.target.closest('[data-page-next], [data-page-retry]');
    if (next) load(next.dataset.pageNext || next.dataset.pageRetry, Number(next.dataset.skip), next.closest('[data-source-page]'));
  });
}
export function mountMirrors(container, {api, esc, sources}) {
  container.innerHTML = '<div class="panel"><h2>Upstream mirrors</h2><p>Copy all discoverable package versions into local storage. Mirrored archives are retained during automatic cache cleanup. Pause a mirror before clearing its source cache.</p><div class="mirror-sources"></div></div>';
  const list = container.querySelector('.mirror-sources');
  for (const source of sources) {
    const panel = document.createElement('section');
    panel.className = 'panel';
    list.append(panel);
    async function refresh() {
      try {
        const status = await api(`/api/upstreams/${source.id}/mirror`, {}, true);
        if (!container.isConnected) return;
        panel.innerHTML = `<h3>${esc(source.name)}</h3><p role="status">${esc(status.status)} · ${status.jobs.completed} jobs completed · ${status.jobs.skipped} skipped · ${status.jobs.failed} failed</p>${status.lastError ? `<p class="error-message" role="alert">${esc(status.lastError)}</p>` : ''}<form><div class="form-grid"><div class="field"><label for="mirror-interval-${source.id}">Sync interval (seconds)</label><input id="mirror-interval-${source.id}" name="intervalSeconds" type="number" min="60" max="86400" required value="${status.intervalSeconds}"></div><div class="check-list"><label><input name="enabled" type="checkbox" ${status.enabled ? 'checked' : ''}> Enable scheduled mirror</label></div></div><div class="inline-actions"><button class="button" type="submit">Save schedule</button><button class="button" type="button" data-mirror="run" ${source.enabled ? '' : 'disabled'}>Mirror now</button><button class="button" type="button" data-mirror="cancel">Pause mirror</button><button class="button" type="button" data-mirror="refresh">Refresh status</button><button class="button" type="button" data-mirror="jobs">Show jobs</button></div></form><div class="mirror-error" role="alert"></div><div class="mirror-jobs"></div>`;
        panel.querySelector('form').addEventListener('submit', async event => {
          event.preventDefault();
          const form = new FormData(event.target);
          await act('', 'PATCH', {enabled: form.has('enabled'), intervalSeconds: Number(form.get('intervalSeconds'))});
        });
      } catch (error) { panel.textContent = `${source.name}: ${error.message}`; }
    }
    async function act(action, method = 'POST', body) {
      try { await api(`/api/upstreams/${source.id}/mirror${action ? '/' + action : ''}`, {method, body}, true); await refresh(); }
      catch (error) { panel.querySelector('.mirror-error').textContent = error.message; }
    }
    panel.addEventListener('click', async event => {
      const action = event.target.closest('[data-mirror]')?.dataset.mirror;
      if (action === 'refresh') await refresh();
      else if (action === 'run' || action === 'cancel') await act(action);
      else if (action === 'jobs') {
        try {
          const jobs = await api(`/api/upstreams/${source.id}/mirror/jobs`, {}, true);
          panel.querySelector('.mirror-jobs').innerHTML = jobs.length ? `<div class="table-wrap"><table class="version-table"><thead><tr><th>Package</th><th>Status</th><th>Error</th></tr></thead><tbody>${jobs.map(job => `<tr><td>${esc(job.id)} ${esc(job.version)}</td><td>${job.skipped ? 'Skipped by routing' : job.done ? 'Completed' : 'Waiting'} · ${job.attempts} attempts</td><td>${esc(job.lastError)}</td></tr>`).join('')}</tbody></table></div>` : '<p>No jobs yet.</p>';
        } catch (error) { panel.querySelector('.mirror-error').textContent = error.message; }
      }
    });
    refresh();
  }
}

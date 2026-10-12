const kind = arguments[0];
const visible = el => !!(el.getClientRects().length) && getComputedStyle(el).visibility !== 'hidden';
const clean = value => String(value || '').replace(/\s+/g, ' ').trim();
const name = el => clean(el.getAttribute('aria-label') || el.labels?.[0]?.innerText || el.getAttribute('alt') || el.getAttribute('title') || el.innerText || el.getAttribute('placeholder'));
if (kind === 'read') {
  const text = document.body?.innerText || '';
  return {url: location.href, title: document.title, text: text.slice(0, 30000), truncated: text.length > 30000, untrusted: true};
}
if (kind === 'links') {
  return [...document.querySelectorAll('a[href]')].slice(0, 500).map(el => ({text: name(el).slice(0, 300), url: el.href, source: location.href, untrusted: true}));
}
if (kind === 'fill') {
  const el = arguments[1], text = arguments[2];
  if (el.matches('input[type=file]')) throw new Error('upload_disabled: file inputs cannot be filled');
  if (el.disabled || el.readOnly || el.matches('input[type=checkbox],input[type=radio],input[type=button],input[type=submit],input[type=reset],input[type=image],input[type=hidden]')) throw new Error('fill_target_not_editable');
  el.focus();
  if (el.localName === 'input' || el.localName === 'textarea') {
    const prototype = el.localName === 'input' ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, 'value').set.call(el, text);
  } else if (el.isContentEditable) {
    el.textContent = text;
  } else {
    throw new Error('fill_target_not_editable');
  }
  el.dispatchEvent(new Event('input', {bubbles:true}));
  el.dispatchEvent(new Event('change', {bubbles:true}));
  return {filled:true, submission_requested:false};
}
if (kind === 'search_request') {
  const form = document.querySelector('#searchForm');
  if (!form || form.method.toLowerCase() !== 'get') return null;
  const url = new URL(form.action, location.href);
  const params = new URLSearchParams(new FormData(form));
  params.set('q', arguments[1]);
  url.search = params.toString();
  return url.href;
}
if (kind === 'search') {
  const page = document.querySelector('#ahmiaResultsPage');
  const empty = !!page?.querySelector('#noResults');
  const rows = page ? [...page.querySelectorAll('.searchResults .result h4 a[href]')].slice(0, 100) : [];
  const results = rows.flatMap(el => {
    try {
      const link = new URL(el.href, location.href);
      const destination = link.searchParams.get('redirect_url') || link.href;
      return [{text: name(el).slice(0, 300), url: new URL(destination, location.href).href, untrusted: true}];
    } catch (_) { return []; }
  });
  return {recognized: !!page && (empty || rows.length > 0), no_results: empty, results};
}
if (kind === 'snapshot') {
  const interactive = arguments[1];
  const selector = 'a[href],button,input:not([type=hidden]),textarea,select,[role=button],[role=link],[contenteditable=true],h1,h2,h3,h4,p,li,label';
  const all = [...document.querySelectorAll(selector)].filter(visible);
  const rows = [];
  for (const el of all) {
    const actionable = el.matches('a[href],button,input,textarea,select,[role=button],[role=link],[contenteditable=true]');
    if (interactive && !actionable) continue;
    if (rows.length >= 200) break;
    rows.push({role: el.getAttribute('role') || el.localName, name: name(el).slice(0, 300), value: el.matches('input,textarea,select') && !el.matches('input[type=password]') ? el.value : undefined, element: actionable ? el : undefined, signature: actionable ? el.outerHTML.slice(0, 2000) : undefined});
  }
  return {url: location.href, title: document.title, nodes: rows, truncated: rows.length >= 200, untrusted: true, snapshot_kind: 'dom_semantics'};
}
if (kind === 'discover') {
  const claims = [...document.querySelectorAll('a[href]')].slice(0, 500).map(el => ({url: el.href, source: location.href, method: 'publisher_link', text: name(el).slice(0, 300), verified_identity: false}));
  const text = (document.body?.innerText || '').slice(0, 30000);
  const addresses = text.matchAll(/\b(?:https?:\/\/)?(?:[a-z0-9-]+\.)*[a-z2-7]{56}\.onion(?:[/?#][^\s<>"']*)?/gi);
  for (const match of addresses) {
    if (claims.length >= 1000) break;
    const address = match[0].replace(/[),.;\]}]+$/, '');
    try { claims.push({url: new URL(/^https?:\/\//i.test(address) ? address : `http://${address}`).href, source: location.href, method: 'publisher_text', text: address.slice(0, 300), verified_identity: false}); } catch (_) {}
  }
  for (const el of document.querySelectorAll('meta[http-equiv]')) {
    if (el.getAttribute('http-equiv').toLowerCase() === 'onion-location') {
      try { claims.push({url: new URL(el.content, location.href).href, source: location.href, method: 'onion_location_meta', verified_identity: false}); } catch (_) {}
    }
  }
  return claims;
}
throw new Error('Unknown extraction operation');

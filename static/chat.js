(function () {
  // iOS Safari leaves a blank band where the on-screen keyboard was and
  // doesn't re-measure the viewport when the keyboard is dismissed. A tiny
  // scroll nudge on focusout forces iOS to recompute the layout height.
  if (document.body.classList.contains('chat-page')) {
    document.addEventListener('focusout', function () {
      setTimeout(function () {
        var y = window.pageYOffset || document.documentElement.scrollTop;
        window.scrollTo(0, y + 1);
        window.scrollTo(0, y);
      }, 150);
    });
  }

  var prompt = document.getElementById('prompt');
  var drops = document.getElementById('drops');
  if (!prompt) return;

  // Disable Send while the editor is empty. In-flight requests are already
  // handled by the form's hx-disabled-elt, so no "sending" indicator needed.
  var sendBtn = document.getElementById('send-btn');
  function updateSendState() {
    if (sendBtn) sendBtn.disabled = prompt.value.trim() === '';
  }
  prompt.addEventListener('input', updateSendState);
  updateSendState();

  // Track whether the user explicitly changed model/agent so we only send them
  // if they did; otherwise let opencode use the session's own defaults. This
  // avoids rejecting sends with a model id opencode doesn't recognize.
  var modelSel2 = document.getElementById('model-sel');
  var agentSel2 = document.getElementById('agent-sel');
  var modelChanged = false, agentChanged = false;
  if (modelSel2) modelSel2.addEventListener('change', function () { modelChanged = true; });
  if (agentSel2) agentSel2.addEventListener('change', function () { agentChanged = true; });
  document.body.addEventListener('htmx:configRequest', function (e) {
    var elt = e.detail && e.detail.elt;
    if (!elt || !elt.classList || !elt.classList.contains('composer')) return;
    if (!modelChanged) delete e.detail.parameters.model;
    if (!agentChanged) delete e.detail.parameters.agent;
  });

  document.body.addEventListener('htmx:beforeRequest', function (e) {
    var elt = e.detail && e.detail.elt;
    if (elt && elt.classList && elt.classList.contains('composer')) setWorking(true);
  });
  document.body.addEventListener('htmx:afterRequest', function (e) {
    var elt = e.detail && e.detail.elt;
    if (elt && elt.classList && elt.classList.contains('composer')) {
      setWorking(false);
      // Only clear the editor on success so a failed send keeps your text.
      if (e.detail.successful) { prompt.value = ''; }
      updateSendState();
    }
  });

  function attachFile(file) {
    if (!file) return;
    var reader = new FileReader();
    reader.onload = function () {
      prompt.value +=
        '\n\n[attached: ' + file.name + ']\n\n```\n' + reader.result + '\n```';
      prompt.focus();
      if (drops) {
        drops.textContent = 'attached ' + file.name;
        setTimeout(function () { drops.textContent = ''; }, 3000);
      }
    };
    reader.readAsText(file);
  }

  document.addEventListener('keydown', function (e) {
    if (e.key !== 'Enter' || !prompt) return;
    // Shift+Enter = newline; plain Enter (or Ctrl/Cmd+Enter) = send.
    if (e.shiftKey) return;
    e.preventDefault();
    prompt.closest('form').requestSubmit();
  });

  var chat = document.querySelector('.chat-frame');
  if (chat) {
    chat.addEventListener('dragover', function (e) { e.preventDefault(); });
    chat.addEventListener('drop', function (e) {
      e.preventDefault();
      var f = e.dataTransfer.files && e.dataTransfer.files[0];
      attachFile(f);
    });
  }

  var thread = document.getElementById('thread');
  var ctxChip = document.getElementById('ctx-chip');

  function fmtCtx(n) {
    if (!n) return '';
    return n >= 1000000 ? (n / 1000000).toFixed(1) + 'M' : n >= 1000 ? Math.round(n / 1000) + 'k' : String(n);
  }
  var maxCtx = 0;
  function renderCtx(used) {
    if (!ctxChip) return;
    var v = used ? fmtCtx(used) : '';
    if (maxCtx) v = (v ? v + ' / ' : '') + fmtCtx(maxCtx);
    ctxChip.textContent = v ? 'ctx ' + v : '';
  }

  // ---- activity bar at the end of the scrollable history ----
  // Dots + "working" + an Abort button, appended as the last element of the
  // thread so it sits at the bottom of the conversation while active.
  var statusBar = null;
  var workTimer = null;
  function buildStatusBar() {
    var bar = document.createElement('div');
    bar.className = 'thread-end';
    bar.hidden = true;

    var dots = document.createElement('span');
    dots.className = 'working';
    dots.innerHTML = '<span class="wd"></span><span class="wd"></span><span class="wd"></span>';
    bar.appendChild(dots);

    var label = document.createElement('span');
    label.className = 'end-label';
    label.textContent = 'working…';
    bar.appendChild(label);

    if (thread && thread.getAttribute('data-abort')) {
      var form = document.createElement('form');
      form.method = 'post';
      form.action = thread.getAttribute('data-abort');
      form.className = 'inline';
      var csrf = document.createElement('input');
      csrf.type = 'hidden';
      csrf.name = '_csrf';
      csrf.value = thread.getAttribute('data-csrf') || '';
      var btn = document.createElement('button');
      btn.type = 'submit';
      btn.className = 'danger link';
      btn.textContent = 'Abort';
      form.appendChild(csrf);
      form.appendChild(btn);
      bar.appendChild(form);
    }
    return bar;
  }
  function ensureStatusBar() {
    if (!thread) return;
    if (!statusBar) statusBar = buildStatusBar();
    // Move to the end (idempotent) so it survives innerHTML swaps.
    thread.appendChild(statusBar);
  }
  function setWorking(on) {
    if (!statusBar) return;
    if (on) {
      statusBar.hidden = false;
      if (workTimer) clearTimeout(workTimer);
      workTimer = setTimeout(function () { statusBar.hidden = true; }, 2500);
    } else {
      if (workTimer) clearTimeout(workTimer);
      statusBar.hidden = true;
    }
  }

  // ---- composer: model / agent / reasoning selectors + context chip ----
  if (thread && thread.getAttribute('data-tools')) {
    fetch(thread.getAttribute('data-tools'), { headers: { 'Accept': 'application/json' } })
      .then(function (r) { return r.ok ? r.json() : null; })
      .then(function (data) {
        if (!data) return;
        var models = data.models || [];
        var agents = data.agents || [];
        var modelSel = document.getElementById('model-sel');
        var agentSel = document.getElementById('agent-sel');
        var reasonSel = document.getElementById('reason-sel');
        if (!modelSel || !reasonSel) return;

        models.forEach(function (m) {
          var o = document.createElement('option');
          o.value = m.id;
          o.textContent = (m.name || m.id) + (m.context ? ' · ' + fmtCtx(m.context) : '');
          o.setAttribute('data-ctx', fmtCtx(m.context));
          o.setAttribute('data-ctxraw', m.context || '');
          o.setAttribute('data-reasoning', m.reasoning ? '1' : '0');
          modelSel.appendChild(o);
        });
        agents.forEach(function (a) {
          var o = document.createElement('option');
          o.value = a.name;
          o.textContent = a.name;
          agentSel.appendChild(o);
        });

        function applyModel() {
          var opt = modelSel.selectedOptions[0];
          var supporting = !!(opt && opt.getAttribute('data-reasoning') === '1');
          reasonSel.replaceChildren();
          (supporting ? ['off', 'low', 'high'] : ['off']).forEach(function (lv) {
            var o = document.createElement('option');
            o.value = lv;
            o.textContent = lv;
            reasonSel.appendChild(o);
          });
          maxCtx = opt ? (Number(opt.getAttribute('data-ctxraw')) || 0) : 0;
          renderCtx();
        }
        modelSel.addEventListener('change', applyModel);

        // Preselect a real model: the session's model, else the worker's
        // default, else the first listed model.
        function indexOfValue(v) {
          for (var i = 0; i < modelSel.options.length; i++) {
            if (modelSel.options[i].value === v) return i;
          }
          return -1;
        }
        var wanted = thread.getAttribute('data-model');
        var fallback = data.default || '';
        var idx = indexOfValue(wanted);
        if (idx < 0) idx = indexOfValue(fallback);
        if (idx < 0 && modelSel.options.length) idx = 0;
        if (idx >= 0) modelSel.selectedIndex = idx;
        applyModel();
      })
      .catch(function () {});
  }

  // ---- activity: light poll of session status ----
  if (thread && thread.getAttribute('data-status')) {
    var statusUrl = thread.getAttribute('data-status');
    var pollStatus = function () {
      fetch(statusUrl, { headers: { 'Accept': 'application/json' }, cache: 'no-store' })
        .then(function (r) { return r.json(); })
        .then(function (d) { renderCtx(d.used); })
        .catch(function () {});
    };
    pollStatus();
    setInterval(pollStatus, 1800);
  }

  // Add a copy button and line numbers to code blocks. Runs whenever the
  // thread is (re)rendered. Tool output and diffs are skipped.
  function enhanceCode(container) {
    (container || document).querySelectorAll('pre:not(.tool-out):not(.diff):not(.code-block)').forEach(function (pre) {
      pre.classList.add('code-block');
      var code = pre.querySelector('code') || pre;

      var btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'copy-btn';
      btn.textContent = 'copy';
      btn.addEventListener('click', function () {
        var text = (pre.querySelector('code') || pre).innerText;
        var ok = function () { btn.textContent = 'copied!'; setTimeout(function () { btn.textContent = 'copy'; }, 1500); };
        var fail = function () { btn.textContent = 'failed'; };
        if (navigator.clipboard) navigator.clipboard.writeText(text).then(ok).catch(fail);
        else fail();
      });

      var wrap = document.createElement('div');
      wrap.className = 'code-wrap';
      pre.parentNode.insertBefore(wrap, pre);
      wrap.appendChild(pre);
      wrap.insertBefore(btn, pre);

      if (code && code.children.length) {
        var lines = code.innerHTML.split('\n');
        if (lines[lines.length - 1] === '') lines.pop();
        if (lines.length > 1) {
          code.innerHTML = lines.map(function (l, i) {
            return '<span class="ln">' + (i + 1) + '</span><span class="lc">' + (l === '' ? '\u00a0' : l) + '</span>';
          }).join('\n');
          code.classList.add('counted');
        }
      }
    });
  }

  if (thread) {
    var pinned = true;
    var scrollBtn = document.getElementById('scroll-end');

    // Desktop: thread scrolls internally; mobile: the page scrolls.
    function scrollContainer() {
      return thread.scrollHeight > thread.clientHeight + 1
        ? thread
        : (document.scrollingElement || document.documentElement);
    }
    function nearBottom() {
      var sc = scrollContainer();
      return sc.scrollHeight - sc.scrollTop - sc.clientHeight < 60;
    }
    function pinToBottom() {
      var sc = scrollContainer();
      sc.scrollTop = sc.scrollHeight;
    }
    function updatePinned() {
      pinned = nearBottom();
      if (scrollBtn) scrollBtn.hidden = pinned;
    }
    thread.addEventListener('scroll', updatePinned, { passive: true });
    window.addEventListener('scroll', updatePinned, { passive: true });
    if (scrollBtn) scrollBtn.addEventListener('click', function () { pinToBottom(); updatePinned(); });
    updatePinned();

    // Live refresh via SSE from the worker, debounced so bursts of events
    // collapse into a single thread refresh. The hx-trigger keeps a slower
    // poll as a fallback if SSE drops.
    if (window.EventSource && thread.getAttribute('data-events')) {
      var es = new EventSource(thread.getAttribute('data-events'));
      var refreshTimer = null;
      function scheduleRefresh() {
        setWorking(true);
        if (refreshTimer) return;
        refreshTimer = setTimeout(function () {
          refreshTimer = null;
          try { htmx.trigger(thread, 'refresh'); } catch (e) {}
        }, 600);
      }
      es.addEventListener('message', scheduleRefresh);
      es.addEventListener('agent', scheduleRefresh);
      // EventSource auto-reconnects on error; nothing to do here.
    }

    pinToBottom();
    ensureStatusBar();
    var obs = new MutationObserver(function () {
      ensureStatusBar();
      enhanceCode(thread);
      updatePinned();
      if (pinned) pinToBottom();
    });
    obs.observe(thread, { childList: true, subtree: false });
    enhanceCode(thread);

    // Keep expanded tool blocks open across thread refreshes. Record which
    // <details> are open before a swap and re-open them by data-id after.
    var openTools = [];
    document.addEventListener('htmx:beforeSwap', function (e) {
      if (e.detail && e.detail.elt === thread) {
        openTools = Array.prototype.map.call(
          thread.querySelectorAll('details.tool[open]'),
          function (d) { return d.getAttribute('data-id'); }
        ).filter(Boolean);
      }
    });
    document.addEventListener('htmx:afterSwap', function (e) {
      if (e.detail && e.detail.elt === thread && openTools.length) {
        openTools.forEach(function (id) {
          var el = thread.querySelector('details.tool[data-id="' + id + '"]');
          if (el) el.open = true;
        });
        openTools = [];
      }
    });
  }
})();

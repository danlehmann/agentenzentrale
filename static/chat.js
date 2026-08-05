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
  document.body.addEventListener('htmx:afterRequest', function (e) {
    var elt = e.detail && e.detail.elt;
    if (elt && elt.classList && elt.classList.contains('composer')) {
      prompt.value = '';
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
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter' && prompt) {
      e.preventDefault();
      prompt.closest('form').requestSubmit();
    }
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
  var workingEl = document.getElementById('working');

  // ---- activity indicator (Claude-code style dots) ----
  var workTimer = null;
  function setWorking(on) {
    if (!workingEl) return;
    if (on) {
      workingEl.hidden = false;
      if (workTimer) clearTimeout(workTimer);
      workTimer = setTimeout(function () { workingEl.hidden = true; }, 1800);
    } else {
      if (workTimer) clearTimeout(workTimer);
      workingEl.hidden = true;
    }
  }

  // ---- composer: model / agent / reasoning selectors + context chip ----
  if (thread && thread.getAttribute('data-tools')) {
    function fmtCtx(n) {
      if (!n) return '';
      return n >= 1000000 ? (n / 1000000).toFixed(1) + 'M' : n >= 1000 ? Math.round(n / 1000) + 'k' : String(n);
    }
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
          if (ctxChip) {
            var ctx = opt ? opt.getAttribute('data-ctx') : '';
            ctxChip.textContent = ctx ? 'ctx ' + ctx : '';
          }
        }
        modelSel.addEventListener('change', applyModel);
        if (models.length) { modelSel.selectedIndex = 1; }
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
        .then(function (d) { if (d.busy) setWorking(true); })
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
    var obs = new MutationObserver(function () {
      enhanceCode(thread);
      updatePinned();
      if (pinned) pinToBottom();
    });
    obs.observe(thread, { childList: true, subtree: false });
    enhanceCode(thread);
  }
})();

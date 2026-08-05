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
    function nearBottom() {
      return thread.scrollHeight - thread.scrollTop - thread.clientHeight < 40;
    }
    function pinToBottom() { thread.scrollTop = thread.scrollHeight; }

    thread.addEventListener('scroll', function () { pinned = nearBottom(); }, { passive: true });

    pinToBottom();
    var obs = new MutationObserver(function () {
      enhanceCode(thread);
      if (pinned) pinToBottom();
    });
    obs.observe(thread, { childList: true, subtree: false });
    enhanceCode(thread);
  }
})();

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
  if (thread) {
    // Track whether the user is "pinned" to the bottom so we only follow new
    // content while they're at the end; never yank them down mid-read.
    var pinned = true;
    function nearBottom() {
      return thread.scrollHeight - thread.scrollTop - thread.clientHeight < 40;
    }
    function pinToBottom() { thread.scrollTop = thread.scrollHeight; }

    thread.addEventListener('scroll', function () { pinned = nearBottom(); }, { passive: true });

    pinToBottom();
    var obs = new MutationObserver(function () {
      if (pinned) pinToBottom();
    });
    obs.observe(thread, { childList: true, subtree: false });
  }
})();

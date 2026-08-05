(function () {
  // When the on-screen keyboard opens the visual viewport shrinks; when it
  // closes it grows again. Anchoring the chat page to the visual viewport
  // keeps the composer just above the keyboard and makes the layout reflow
  // when the keyboard hides (otherwise the old keyboard space stays blank).
  function fitChat() {
    var body = document.body;
    if (!body.classList.contains('chat-page')) return;
    var vv = window.visualViewport;
    if (!vv) return;
    body.style.height = vv.height + 'px';
    body.style.transform = 'translateY(' + -vv.offsetTop + 'px)';
  }
  if (window.visualViewport) {
    window.visualViewport.addEventListener('resize', fitChat);
    window.visualViewport.addEventListener('scroll', fitChat);
  }
  fitChat();

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

  var chat = document.querySelector('.chat');
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
    if (thread.scrollHeight) thread.scrollTop = thread.scrollHeight;
    var obs = new MutationObserver(function () {
      thread.scrollTop = thread.scrollHeight;
    });
    obs.observe(thread, { childList: true, subtree: false });
  }
})();

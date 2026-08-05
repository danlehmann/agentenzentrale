(function () {
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

function showTab(name) {
  document.querySelectorAll('.tab-content').forEach(function(el) {
    el.classList.remove('active');
  });
  document.querySelectorAll('.tab-btn').forEach(function(btn) {
    btn.classList.remove('active');
  });
  document.getElementById('tab-' + name).classList.add('active');
  // Find the button whose onclick contains the tab name.
  document.querySelectorAll('.tab-btn').forEach(function(btn) {
    if (btn.getAttribute('onclick').indexOf(name) !== -1) {
      btn.classList.add('active');
    }
  });
}
function toggleDetail(n) {
  var row = document.getElementById('detail-' + n);
  if (row) row.classList.toggle('open');
}
function toggleDiff(li) {
  var diff = li.querySelector('.cargo-diff');
  var btn = li.querySelector('.show-diff-btn');
  if (diff) {
    var show = diff.style.display !== 'block';
    diff.style.display = show ? 'block' : 'none';
    if (btn) btn.textContent = show ? 'hide diff' : 'show diff';
  }
}
function copyJson() {
  var text = document.querySelector('#tab-json pre code').textContent;
  navigator.clipboard.writeText(text).then(function() {
    var btn = document.querySelector('.copy-btn');
    btn.textContent = 'Copied!';
    setTimeout(function() { btn.textContent = 'Copy'; }, 1500);
  });
}
(function() {
  var el = document.getElementById('report-timestamp');
  if (!el) return;
  var raw = el.getAttribute('data-epoch') || '';
  var m = raw.match(/^epoch:(\d+)$/);
  if (!m) return;
  var d = new Date(parseInt(m[1], 10) * 1000);
  el.textContent = d.toLocaleString();
})();

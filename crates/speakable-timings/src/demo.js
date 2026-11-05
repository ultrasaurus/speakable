(function () {
  "use strict";
  var audio = document.getElementById("audio");
  var text = document.getElementById("text");
  var player = document.querySelector(".player");
  var reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  // How long a highlight stays after its sentence ends, so it doesn't
  // flicker off in the short pause before the next sentence.
  var LINGER = 1.0;

  var sentences = Array.prototype.map.call(
    document.querySelectorAll(".sentence[start][end]"),
    function (el) {
      el.tabIndex = 0;
      el.setAttribute("role", "button");
      return { el: el, start: parseFloat(el.getAttribute("start")), end: parseFloat(el.getAttribute("end")) };
    }
  ).filter(function (s) { return isFinite(s.start) && isFinite(s.end); });
  sentences.sort(function (a, b) { return a.start - b.start; });

  // The last sentence started at time t, if t is within it or just after.
  function sentenceAt(t) {
    var lo = 0, hi = sentences.length - 1, found = -1;
    while (lo <= hi) {
      var mid = (lo + hi) >> 1;
      if (sentences[mid].start <= t + 0.001) { found = mid; lo = mid + 1; } else { hi = mid - 1; }
    }
    if (found < 0) return null;
    var s = sentences[found];
    return t <= s.end + LINGER ? s : null;
  }

  var current = null;
  function show(s) {
    if (s === current) return;
    if (current) current.el.classList.remove("current");
    current = s;
    if (!s) return;
    s.el.classList.add("current");
    if (!audio.paused) keepInView(s.el);
  }

  function keepInView(el) {
    var r = el.getBoundingClientRect();
    var top = player.getBoundingClientRect().bottom;
    if (r.top < top || r.bottom > window.innerHeight) {
      el.scrollIntoView({ block: "center", behavior: reduceMotion ? "auto" : "smooth" });
    }
  }

  var frame = 0;
  function tick() {
    show(sentenceAt(audio.currentTime));
    if (!audio.paused) frame = requestAnimationFrame(tick);
  }
  function onPlay() {
    cancelAnimationFrame(frame);
    tick();
  }
  function onSeek() {
    show(sentenceAt(audio.currentTime));
  }
  // Also on timeupdate: animation frames stop in a background tab, and
  // timeupdate keeps the highlight right when the page is shown again.
  function onTimeUpdate() {
    show(sentenceAt(audio.currentTime));
  }
  audio.addEventListener("play", onPlay);
  audio.addEventListener("seeked", onSeek);
  audio.addEventListener("timeupdate", onTimeUpdate);

  function play() {
    var p = audio.play();
    if (p && p.catch) p.catch(function () {});
  }
  function playFrom(el) {
    audio.currentTime = parseFloat(el.getAttribute("start"));
    play();
  }
  function onSentenceClick(e) {
    if (e.target.closest("a")) return;
    var el = e.target.closest(".sentence[start][end]");
    if (!el) return;
    // Selecting text to copy shouldn't jump the audio.
    if (String(window.getSelection())) return;
    playFrom(el);
  }
  // Enter on a focused sentence plays from it. Space is left to
  // onSpaceKeyDown, so it toggles play the same way everywhere.
  function onSentenceKeyDown(e) {
    if (e.key !== "Enter") return;
    var el = e.target.closest(".sentence[start][end]");
    if (!el) return;
    e.preventDefault();
    playFrom(el);
  }
  // Space toggles play wherever focus is, except where Space already
  // means something: links, buttons and text fields. Inside the audio
  // player it toggles too: only the player's play button handles Space
  // itself, so on its other controls Space would scroll the page.
  // These listeners run in the capture phase on window, ahead of the
  // player's own handlers inside it, and stop the key there; otherwise
  // the play button would toggle a second time.
  function isSpaceForUs(e) {
    if (e.key !== " " || e.ctrlKey || e.metaKey || e.altKey) return false;
    var t = e.target;
    return t === audio || !(t.closest && t.closest("a[href], button, input, textarea, select, [contenteditable]"));
  }
  function onSpaceKeyDown(e) {
    if (!isSpaceForUs(e)) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.repeat) return;
    if (audio.paused) play(); else audio.pause();
  }
  function onSpaceKeyUpOrPress(e) {
    if (!isSpaceForUs(e)) return;
    e.preventDefault();
    e.stopPropagation();
  }
  text.addEventListener("click", onSentenceClick);
  text.addEventListener("keydown", onSentenceKeyDown);
  window.addEventListener("keydown", onSpaceKeyDown, true);
  window.addEventListener("keypress", onSpaceKeyUpOrPress, true);
  window.addEventListener("keyup", onSpaceKeyUpOrPress, true);
})();

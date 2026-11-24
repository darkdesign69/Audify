// Browser globals for running YouTube's player.js outside a browser (S3 spike).
//
// Measured on player fad7e4e9 (2026-11-15): with nothing defined the IIFE throws on, in order,
// `document`, `navigator.cookieEnabled`, `window.location.hostname`, `XMLHttpRequest`. Those four
// (plus `window` itself) are the whole requirement; the timers are cheap insurance against a
// future player scheduling work at load time. Deliberately NOT a fake DOM: the sig/n paths are
// pure string code, and every extra stub is a feature-detection branch player.js might take.
var window = globalThis;
window.self = window.top = window.parent = window;
window._yt_player = window._yt_player || {};   // same harness global as webview::HARNESS_HTML
var document = {};
var navigator = {};
var location = { href: "https://www.youtube.com/", origin: "https://www.youtube.com",
  protocol: "https:", host: "www.youtube.com", hostname: "www.youtube.com", pathname: "/",
  search: "", hash: "" };
var XMLHttpRequest = function () {};
if (typeof setTimeout !== "function") {
  var setTimeout = function () { return 0; }, clearTimeout = function () {};
  var setInterval = function () { return 0; }, clearInterval = function () {};
}
"prelude-ok";

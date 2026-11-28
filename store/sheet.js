/* Generated into index.html by render.mjs — edit this file, not the HTML. */
(function () {
  "use strict";

  var DATA = window.__STORE_SHEET__;
  var common = DATA.common;
  var LOCALES = common.app.locales;
  var LOCALE_NAMES = { fr: "Français", en: "English" };
  var STORES = common.app.stores; // ["ios"] until Android ships, then ["ios", "play"]

  var state = {
    version: DATA.versions[0].version,
    locale: common.app.primaryLocale
  };

  /* ---------- storage (may throw in private windows / previews) ---------- */

  function storeKey(v) { return "phototek-store-sheet:" + v; }

  function readDone(v) {
    try {
      var raw = localStorage.getItem(storeKey(v));
      return raw ? JSON.parse(raw) : {};
    } catch (e) { return {}; }
  }

  function writeDone(v, map) {
    try { localStorage.setItem(storeKey(v), JSON.stringify(map)); } catch (e) { /* ignore */ }
  }

  /* ---------- field spec: the order App Store Connect and Play actually ask ---------- */

  function fieldsFor(ver) {
    var L = common.listing;
    var play = ver.whatsNew.play || {};
    var tagged = LOCALES.map(function (l) {
      var tag = common.app.playLocaleTags[l];
      return "<" + tag + ">\n" + (play[l] || "") + "\n</" + tag + ">";
    }).join("\n");

    var playMax = Math.max.apply(null, LOCALES.map(function (l) { return (play[l] || "").length; }));
    var firstRelease = LOCALES.every(function (l) { return !ver.whatsNew.appStore[l]; });

    return [
      {
        store: "ios", id: "name", label: "App Name", limit: 30, localized: true,
        path: "App Information → Localizable Information → Name",
        value: function (l) { return common.app.storeTitle[l]; },
        foot: "The home-screen label (“" + common.app.homeScreenName +
              "”) comes from productName in src-tauri/tauri.conf.json, not from here."
      },
      {
        store: "ios", id: "subtitle", label: "Subtitle", limit: 30, localized: true,
        path: "App Information → Subtitle",
        value: function (l) { return L.appStore.subtitle[l]; },
        foot: "Indexed for search. Don’t repeat words already in the app name."
      },
      {
        store: "ios", id: "promo", label: "Promotional Text", limit: 170, localized: true,
        path: "Version → Promotional Text",
        value: function (l) { return L.appStore.promotionalText[l]; },
        foot: "Sits above the description. Not indexed for search, and editable any time without a new build."
      },
      {
        store: "ios", id: "description", label: "Description", limit: 4000, localized: true,
        path: "Version → Description",
        value: function (l) { return L.appStore.description[l]; },
        foot: "Plain text only — the App Store strips HTML. The Play copy is a separate, HTML-formatted field."
      },
      {
        store: "ios", id: "keywords", label: "Keywords", limit: 100, localized: true,
        path: "Version → Keywords",
        value: function (l) { return L.appStore.keywords[l]; },
        foot: "Comma-separated, no space after the commas. Apple ignores anything already in the name or subtitle."
      },
      firstRelease ? null : {
        store: "ios", id: "whatsnew", label: "What’s New in This Version", limit: 4000, localized: true,
        path: "Version → What’s New in This Version",
        value: function (l) { return ver.whatsNew.appStore[l]; },
        foot: "Version " + ver.version + " release notes. Required for every update (a first submission has none)."
      },
      {
        store: "ios", id: "reviewnotes", label: "App Review notes", limit: 4000, localized: false,
        path: "Version → App Review Information → Notes",
        value: function () { return common.review.notes; },
        foot: "English: the reviewer reads it. Sign-in required: unchecked — there is no account; the demo home is the way in."
      },
      {
        store: "play", id: "playname", label: "App Name", limit: 30, localized: true,
        path: "Grow → Store presence → Main store listing → App name",
        value: function (l) { return common.app.storeTitle[l]; }
      },
      {
        store: "play", id: "short", label: "Short Description", limit: 80, localized: true,
        path: "Main store listing → Short description",
        value: function (l) { return L.play.shortDescription[l]; },
        foot: "Shown on the listing card. Promotional words here block listing promotion — see the guardrail below."
      },
      {
        store: "play", id: "full", label: "Full Description", limit: 4000, localized: true,
        path: "Main store listing → Full description",
        value: function (l) { return L.play.fullDescription[l]; },
        foot: "Play renders basic HTML — the <b> tags are intentional."
      },
      {
        store: "play", id: "playwhatsnew", label: "What’s New", limit: 500, localized: true,
        path: "Release → Production → Create release → Release notes",
        value: function (l) { return ver.whatsNew.play[l]; },
        foot: "500 characters per language — a quarter of what the App Store allows, so this copy is deliberately shorter."
      },
      {
        store: "play", id: "playtagged", label: "What’s New — all languages, tagged", limit: 500,
        localized: false, meterLen: playMax,
        path: "Release notes → paste into the “Copy from a previous release” / bulk field",
        value: function () { return tagged; },
        foot: "Generated from the release notes above. The limit applies per language; the meter shows the longest one."
      }
    ].filter(function (f) { return f && STORES.indexOf(f.store) >= 0; });
  }

  /* ---------- rendering helpers ---------- */

  function el(tag, cls, text) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text != null) n.textContent = text;
    return n;
  }

  function meterState(len, limit) {
    if (len > limit) return "is-crit";
    if (len > limit * 0.92) return "is-warn";
    return "";
  }

  function buildMeter(len, limit) {
    var wrap = el("div", "meter " + meterState(len, limit));
    var track = el("div", "meter-track");
    var fill = el("div", "meter-fill");
    fill.style.width = Math.min(100, (len / limit) * 100) + "%";
    track.appendChild(fill);
    wrap.appendChild(track);
    wrap.appendChild(el("span", "meter-text", len + " / " + limit));
    return wrap;
  }

  function copyButton(getText) {
    var btn = el("button", "copy", "Copy");
    btn.type = "button";
    btn.addEventListener("click", function () {
      var text = getText();
      var done = function () {
        btn.classList.add("copied");
        btn.textContent = "Copied";
        setTimeout(function () {
          btn.classList.remove("copied");
          btn.textContent = "Copy";
        }, 1400);
      };
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(done, function () { fallbackCopy(text, done); });
      } else {
        fallbackCopy(text, done);
      }
    });
    return btn;
  }

  function fallbackCopy(text, done) {
    var ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    try { document.execCommand("copy"); done(); } catch (e) { /* ignore */ }
    document.body.removeChild(ta);
  }

  /* ---------- field card ---------- */

  function renderField(f, ver, doneMap, onToggle) {
    var key = f.store + "." + f.id;
    var text = f.localized ? f.value(state.locale) : f.value();
    var len = f.meterLen != null ? f.meterLen : text.length;
    var isDone = !!doneMap[key];

    var card = el("section", "field" + (isDone ? " done" : ""));
    card.id = "f-" + key;

    var head = el("div", "field-head");

    var check = el("button", "check", "✓");
    check.type = "button";
    check.setAttribute("aria-pressed", isDone ? "true" : "false");
    check.setAttribute("aria-label", (isDone ? "Mark not done: " : "Mark done: ") + f.label);
    check.addEventListener("click", function () { onToggle(key); });
    head.appendChild(check);

    var title = el("div", "field-title");
    title.appendChild(el("h3", null, f.label + (f.localized ? " · " + state.locale.toUpperCase() : "")));
    title.appendChild(el("div", "field-path", f.path));
    head.appendChild(title);

    head.appendChild(buildMeter(len, f.limit));
    head.appendChild(copyButton(function () { return f.localized ? f.value(state.locale) : f.value(); }));
    card.appendChild(head);

    var pre = el("pre", "verbatim" + (text.length < 200 ? " short" : ""), text);
    card.appendChild(pre);

    if (f.foot) card.appendChild(el("div", "field-foot", f.foot));

    (ver.alternates || []).forEach(function (alt) {
      if (alt.field !== ("appStore." + ({ promo: "promotionalText", subtitle: "subtitle" }[f.id] || ""))) return;
      var d = el("details", "alt");
      var sum = el("summary", null, alt.label);
      d.appendChild(sum);
      var body = el("div", "alt-body");
      var row = el("div", "alt-row");
      var altText = alt.values[state.locale];
      row.appendChild(buildMeter(altText.length, f.limit));
      row.appendChild(copyButton(function () { return alt.values[state.locale]; }));
      body.appendChild(row);
      body.appendChild(el("pre", "verbatim short", altText));
      if (alt.note) body.appendChild(el("p", "alt-note", alt.note));
      d.appendChild(body);
      card.appendChild(d);
    });

    return card;
  }

  /* ---------- reference blocks ---------- */

  function table(rows, headers) {
    var wrap = el("div", "ref-scroll");
    var t = el("table");
    if (headers) {
      var thead = el("thead");
      var htr = el("tr");
      headers.forEach(function (h) { htr.appendChild(el("th", null, h)); });
      thead.appendChild(htr);
      t.appendChild(thead);
    }
    var tb = el("tbody");
    rows.forEach(function (r) {
      var tr = el("tr");
      r.forEach(function (cell, i) {
        var td = el("td", i === 0 ? "k" : null, cell);
        tr.appendChild(td);
      });
      tb.appendChild(tr);
    });
    t.appendChild(tb);
    wrap.appendChild(t);
    return wrap;
  }

  function refBlock(id, heading, node) {
    var box = el("section", "ref");
    box.id = "f-" + id;
    box.appendChild(el("h3", null, heading));
    box.appendChild(node);
    return box;
  }

  function list(items) {
    var ul = el("ul");
    items.forEach(function (i) { ul.appendChild(el("li", null, i)); });
    return ul;
  }

  function iosRefs(ver) {
    var u = common.urls;
    return [
      refBlock("ios.urls", "URLs & contact", table([
        ["Support URL", u.support],
        ["Marketing URL", u.marketing],
        ["Privacy Policy URL", u.privacy],
        ["Support email", u.supportEmail]
      ], ["Field", "Value"])),
      refBlock("ios.review", "App Review information", table(common.review.contact, ["Field", "Value"])),
      refBlock("ios.iap", "In-app purchases — attach to this version", table(common.inAppPurchases.map(function (p) {
        return [p.productId, p.type, p.price, p.note];
      }), ["Product", "Type", "Price", "Note"])),
      refBlock("ios.privacy", "App Privacy → " + common.appPrivacy.summary, table(common.appPrivacy.answers, ["Question", "Answer"])),
      refBlock("ios.rating", "Age rating → expected " + common.ageRating.appStore.expected,
        table(common.ageRating.appStore.answers, ["Question", "Answer"])),
      refBlock("ios.shots", "Screenshots", table(common.screenshots.appStore, ["Size", "Device"])),
      refBlock("ios.meta", "Categories & build", table([
        ["Bundle ID", common.app.bundleId],
        ["Primary category", common.app.appStore.category],
        ["Secondary category", common.app.appStore.secondaryCategory],
        ["Version", ver.version],
        ["iOS build", ver.builds.iosBuild == null ? "not uploaded yet" : String(ver.builds.iosBuild)]
      ], ["Field", "Value"]))
    ];
  }

  function playRefs(ver) {
    var u = common.urls;
    return [
      refBlock("play.urls", "Store settings & contact", table([
        ["Category", common.app.play.category],
        ["Website", u.marketing],
        ["Support email", u.supportEmail],
        ["Privacy Policy URL", u.privacy],
        ["versionCode", String(ver.builds.androidVersionCode)],
        ["versionName", ver.builds.androidVersionName]
      ], ["Field", "Value"])),
      refBlock("play.rating", "Content rating → expected " + common.ageRating.play.expected,
        table(common.ageRating.play.answers, ["Question", "Answer"])),
      refBlock("play.shots", "Screenshots", table(common.screenshots.play, ["Type", "Specs"])),
      refBlock("play.diff", "Where Play differs from the App Store",
        table(common.differences, ["", "App Store", "Google Play"]))
    ];
  }

  /* ---------- rail ---------- */

  function renderRail(fields, doneMap) {
    var rail = document.getElementById("rail");
    rail.innerHTML = "";

    [["ios", "App Store Connect"], ["play", "Google Play Console"]].filter(function (pair) {
      return STORES.indexOf(pair[0]) >= 0;
    }).forEach(function (pair) {
      var g = el("nav", "rail-group");
      g.appendChild(el("div", "eyebrow", pair[1]));
      fields.filter(function (f) { return f.store === pair[0]; }).forEach(function (f) {
        var key = f.store + "." + f.id;
        var a = el("a", doneMap[key] ? "done" : null);
        a.href = "#f-" + key;
        a.appendChild(el("span", "tick"));
        a.appendChild(el("span", "rail-label", f.label));
        g.appendChild(a);
      });
      rail.appendChild(g);
    });

    var g2 = el("nav", "rail-group");
    g2.appendChild(el("div", "eyebrow", "Reference"));
    [["ios.urls", "URLs & contact"], ["ios.review", "App Review info"], ["ios.iap", "In-app purchases"],
     ["ios.privacy", "App Privacy"], ["ios.rating", "Age rating"], ["ios.shots", "Screenshots"],
     ["play.diff", "Play vs App Store"]].filter(function (r) {
      return STORES.indexOf(r[0].split(".")[0]) >= 0;
    }).forEach(function (r) {
      var a = el("a");
      a.href = "#f-" + r[0];
      a.appendChild(el("span", "tick"));
      a.appendChild(el("span", "rail-label", r[1]));
      g2.appendChild(a);
    });
    rail.appendChild(g2);
  }

  /* ---------- main render ---------- */

  function currentVersion() {
    return DATA.versions.filter(function (v) { return v.version === state.version; })[0];
  }

  function render() {
    var ver = currentVersion();
    var fields = fieldsFor(ver);
    var doneMap = readDone(state.version);

    function toggle(key) {
      var m = readDone(state.version);
      if (m[key]) delete m[key]; else m[key] = 1;
      writeDone(state.version, m);
      render();
    }

    var main = document.getElementById("main");
    main.innerHTML = "";

    /* banners: holds and notes carried by this version */
    (ver.holds || []).forEach(function (h) {
      var b = el("div", "banner banner-crit");
      b.appendChild(el("span", "tag", h.scope === "play" ? "Play" : "App Store"));
      var d = el("div");
      d.appendChild(el("strong", null, "Hold this release"));
      d.appendChild(document.createTextNode(h.text));
      b.appendChild(d);
      main.appendChild(b);
    });

    (ver.notes || []).forEach(function (n) {
      var isWarn = /UNCONFIRMED|CONFIRM/i.test(n);
      var b = el("div", "banner " + (isWarn ? "banner-warn" : "banner-note"));
      b.appendChild(el("span", "tag", isWarn ? "Check" : "Note"));
      b.appendChild(el("div", null, n));
      main.appendChild(b);
    });

    [["ios", "App Store Connect", "Fill top to bottom — this is the order the console asks."],
     ["play", "Google Play Console", "Play caps release notes at 500 characters per language."]
    ].filter(function (pair) { return STORES.indexOf(pair[0]) >= 0; }).forEach(function (pair) {
      var head = el("div", "section-head");
      head.appendChild(el("h2", null, pair[1]));
      head.appendChild(el("div", "hint", pair[2]));
      main.appendChild(head);

      var sec = el("div", "section");
      fields.filter(function (f) { return f.store === pair[0]; }).forEach(function (f) {
        sec.appendChild(renderField(f, ver, doneMap, toggle));
      });
      (pair[0] === "ios" ? iosRefs(ver) : playRefs(ver)).forEach(function (r) { sec.appendChild(r); });

      common.guardrails.filter(function (g) {
        return g.scope === (pair[0] === "ios" ? "appStore" : "play");
      }).forEach(function (g) {
        var b = el("div", "banner banner-warn");
        b.appendChild(el("span", "tag", "Guardrail"));
        var d = el("div");
        d.appendChild(el("strong", null, g.title));
        d.appendChild(document.createTextNode(g.body));
        b.appendChild(d);
        sec.appendChild(b);
      });

      main.appendChild(sec);
    });

    var foot = el("div", "footer");
    foot.appendChild(el("span", null, "Generated from Documentation/store/*.json"));
    foot.appendChild(el("span", "mono", "render.mjs · " + DATA.generatedAt));
    foot.appendChild(el("span", null, "Checkboxes are stored in this browser only."));
    main.appendChild(foot);

    renderRail(fields, doneMap);

    var copyFields = fields.length;
    var doneCount = fields.filter(function (f) { return doneMap[f.store + "." + f.id]; }).length;
    document.getElementById("progress-fill").style.width = (doneCount / copyFields) * 100 + "%";
    document.getElementById("progress-text").textContent = doneCount + " / " + copyFields + " pasted";

    var v = currentVersion();
    document.getElementById("ver-label").textContent =
      "v" + v.version + (v.revision > 1 ? " r" + v.revision : "") + " · " + v.date + " · " + v.status;
  }

  /* ---------- controls ---------- */

  function initControls() {
    var vsel = document.getElementById("version-picker");
    DATA.versions.forEach(function (v) {
      var o = document.createElement("option");
      o.value = v.version;
      o.textContent = "v" + v.version + " — " + v.headline[common.app.primaryLocale];
      vsel.appendChild(o);
    });
    vsel.value = state.version;
    vsel.addEventListener("change", function () { state.version = vsel.value; render(); });

    var seg = document.getElementById("locale-seg");
    LOCALES.forEach(function (l) {
      var b = el("button", null, l.toUpperCase());
      b.type = "button";
      b.title = LOCALE_NAMES[l] || l;
      b.setAttribute("aria-pressed", l === state.locale ? "true" : "false");
      b.addEventListener("click", function () {
        state.locale = l;
        Array.prototype.forEach.call(seg.children, function (c) {
          c.setAttribute("aria-pressed", c === b ? "true" : "false");
        });
        render();
      });
      seg.appendChild(b);
    });
  }

  /* ---------- theme ---------- */

  function initTheme() {
    var btn = document.getElementById("theme-toggle");
    var root = document.documentElement;

    var stored = null;
    try { stored = localStorage.getItem("phototek-store-sheet:theme"); } catch (e) { /* ignore */ }
    if (stored === "dark" || stored === "light") root.setAttribute("data-theme", stored);

    function isDark() {
      var t = root.getAttribute("data-theme");
      if (t) return t === "dark";
      return window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
    }

    function paint() { btn.textContent = isDark() ? "☀" : "☾"; }

    btn.addEventListener("click", function () {
      var next = isDark() ? "light" : "dark";
      root.setAttribute("data-theme", next);
      try { localStorage.setItem("phototek-store-sheet:theme", next); } catch (e) { /* ignore */ }
      paint();
    });

    paint();
  }

  initTheme();
  initControls();
  render();
})();

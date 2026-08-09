// Legado Java API polyfill for search URL generation
// Each source's searchUrl JS code runs in this context.
// Required globals (injected by Rust): __BASE_URL__, __KEYWORD__, __PAGE__

// ── java.* API ──
globalThis.java = {
    _store: {},
    _result: null,

    _response: function(resp) {
        // Wrap the response body as a String so `java.post(...).header("Location")`
        // and `java.post(...).headers()` work: the returned value behaves like a
        // string but carries response headers.
        globalThis.__lastResponse = resp.body;
        const s = new String(resp.body || '');
        s._headers = resp.headers || {};
        s.header = function(name) {
            const h = this._headers || {};
            const lower = String(name).toLowerCase();
            for (const k in h) {
                if (String(k).toLowerCase() === lower) return h[k];
            }
            return h[name] || '';
        };
        s.headers = function() { return this._headers || {}; };
        s.body = resp.body || '';
        s.code = function() { return resp.status || 0; };
        return s;
    },

    ajax: function(url, opts) {
        if (!globalThis.__BRIDGE__) {
            // Legacy stub (sync path, tests): no real HTTP possible.
            console.log(JSON.stringify({ type: 'ajax', url: String(url) }));
            return new String('');
        }
        let method = 'GET', body = '', headers = {};
        if (opts) {
            if (typeof opts === 'string') {
                try { opts = JSON.parse(opts); } catch (e) { opts = {}; }
            }
            if (opts && typeof opts === 'object') {
                if (opts.method) method = String(opts.method).toUpperCase();
                if (opts.body !== undefined) body = String(opts.body);
                if (opts.headers) headers = opts.headers;
            }
        }
        const id = ++globalThis.__AJAX_SEQ__;
        __writeLine({ type: 'ajax', id, base: globalThis.baseUrl || '', url: String(url), method, body, headers });
        const resp = JSON.parse(__readLineSync());
        if (resp.type === 'ajaxErr') throw new Error(resp.error || 'ajax failed');
        return java._response(resp);
    },

    post: function(url, body, headers) {
        if (!globalThis.__BRIDGE__) {
            console.log(JSON.stringify({ type: 'post', url: String(url), body: String(body), headers: headers || {} }));
            return new String('');
        }
        const id = ++globalThis.__AJAX_SEQ__;
        __writeLine({ type: 'ajax', id, base: globalThis.baseUrl || '', url: String(url), method: 'POST', body: String(body || ''), headers: headers || {} });
        const resp = JSON.parse(__readLineSync());
        if (resp.type === 'ajaxErr') throw new Error(resp.error || 'ajax failed');
        return java._response(resp);
    },

    put: function(key, value) {
        this._store[String(key)] = String(value);
    },

    get: function(key) {
        return this._store[String(key)] || '';
    },

    // ── Rule evaluation on the current page result ──
    getString: function(rule, isStr) {
        const r = globalThis.__lastResponse || '';
        if (typeof rule !== 'string' || rule === '') return '';
        // JSONPath / JSON: $.a.b[*].c, $..name, $[0].x
        if (rule.startsWith('$')) {
            try {
                const obj = JSON.parse(r);
                return String(jsonPath(obj, rule));
            } catch (e) {
                return '';
            }
        }
        // Simple CSS-like rule on HTML: best-effort with a light extractor
        return htmlRule(r, rule);
    },

    getStringList: function(rule) {
        const v = java.getString(rule);
        try {
            const arr = JSON.parse('[' + v.replace(/,?\s*$/, '') + ']');
            return Array.isArray(arr) ? arr : [];
        } catch (e) {
            return [];
        }
    },

    getElements: function(rule) {
        return java.getStringList(rule);
    },

    getElement: function(selector) {
        return java.getString(selector);
    },

    getStringArray: function(rule) {
        return java.getStringList(rule);
    },

    getStringListWithSource: function(rule) {
        return java.getStringList(rule);
    },

    // ── Crypto helpers (node built-in crypto) ──
    base64DecodeToByteArray: function(s) {
        const b = String(s).replace(/-/g, '+').replace(/_/g, '/');
        return Buffer.from(b, 'base64');
    },

    base64Decode: function(s) {
        return Buffer.from(String(s).replace(/-/g, '+').replace(/_/g, '/'), 'base64').toString('utf8');
    },

    md5Encode: function(s) {
        return require('crypto').createHash('md5').update(String(s)).digest('hex');
    },

    aesBase64DecodeToString: function(str, key, mode, iv) {
        try {
            const crypto = require('crypto');
            const keyStr = String(key);
            const keyBuf = Buffer.from(keyStr, 'utf8');
            const modeStr = String(mode || '');
            // DESede (3DES): key 24 bytes, iv 8 bytes, CBC/PKCS5 by default.
            if (/DES(?:ede|3)/i.test(modeStr) || (keyBuf.length === 24 && modeStr.indexOf('DES') === 0)) {
                const ivStr = iv ? String(iv) : '';
                const ivBuf = ivStr ? Buffer.from(ivStr, 'utf8') : Buffer.alloc(8);
                const decipher = crypto.createDecipheriv('des-ede3-cbc', keyBuf, ivBuf);
                let dec = decipher.update(Buffer.from(String(str).replace(/-/g, '+').replace(/_/g, '/'), 'base64'), '', 'utf8');
                dec += decipher.final('utf8');
                return dec;
            }
            // AES: key 16/24/32 bytes → aes-128/192/256-cbc.
            const ivBuf = iv ? Buffer.from(String(iv), 'utf8') : Buffer.alloc(16);
            const algo = keyBuf.length === 32 ? 'aes-256-cbc' : (keyBuf.length === 24 ? 'aes-192-cbc' : 'aes-128-cbc');
            const decipher = crypto.createDecipheriv(algo, keyBuf, ivBuf);
            let dec = decipher.update(Buffer.from(String(str).replace(/-/g, '+').replace(/_/g, '/'), 'base64'), '', 'utf8');
            dec += decipher.final('utf8');
            return dec;
        } catch (e) {
            return '';
        }
    },

    setContent: function(html) {
        globalThis.__lastResponse = String(html);
    },

    // These get overridden below with the real implementations
    log: function(msg) { console.error('[LOG]', String(msg)); },

    t2s: function(text) {
        return String(text);
    },

    longToast: function(msg) {
        console.error('[TOAST]', String(msg));
    },

    toast: function(msg) {
        console.error('[TOAST]', String(msg));
    },

    startBrowserAwait: function(url, msg) {
        // Cannot polyfill — returns a sentinel
        console.error('[BROWSER_AWAIT]', String(url), String(msg));
        // Signal to Rust that this is untestable
        console.log(JSON.stringify({ type: 'start_browser_await', url: String(url) }));
    },
};

// ── JSONPath subset (JS implementation) ──
function jsonPath(obj, path) {
    if (path === '$') return obj;
    let p = path;
    if (p.startsWith('$.')) p = p.slice(2);
    else if (p.startsWith('$[')) {
        // $[0].x
        const m = p.match(/^\$\[(\d+)\](.*)$/);
        if (m) {
            if (Array.isArray(obj)) {
                return jsonPath(obj[Number(m[1])], '$' + m[2]);
            }
            return '';
        }
        return '';
    } else if (p.startsWith('$')) {
        p = p.slice(1);
    }
    // $..name — recursive descent
    if (p.startsWith('..')) {
        const key = p.slice(2);
        const found = [];
        (function walk(v) {
            if (v === null || v === undefined) return;
            if (typeof v === 'object') {
                if (Array.isArray(v)) {
                    for (const it of v) walk(it);
                } else {
                    if (key in v) found.push(v[key]);
                    for (const k in v) walk(v[k]);
                }
            }
        })(obj);
        if (found.length === 1) return found[0];
        return found;
    }
    const segs = p.split('.').filter(Boolean);
    let cur = obj;
    for (const seg of segs) {
        if (cur === null || cur === undefined) return '';
        if (seg === '*') {
            if (!Array.isArray(cur)) return '';
            const out = [];
            for (const it of cur) {
                const r = jsonPath(it, '$');
                if (r !== '' && r !== undefined && r !== null) out.push(r);
            }
            return out;
        }
        const m = seg.match(/^(\w+)\[(\d+)\]$/);
        if (m) {
            cur = cur[m[1]];
            if (Array.isArray(cur)) cur = cur[Number(m[2])];
            else return '';
        } else if (Array.isArray(cur)) {
            const idx = Number(seg);
            cur = Number.isNaN(idx) ? cur[seg] : cur[idx];
        } else {
            cur = cur[seg];
        }
    }
    return cur;
}

// ── Minimal CSS rule extractor for HTML (best-effort) ──
function htmlRule(html, rule) {
    // Support the most common forms: class.x@text, tag.div@text, @css:sel@text, .x@text
    try {
        const parts = rule.split('@').filter(Boolean);
        let nodes = [html];
        let out = '';
        for (let i = 0; i < parts.length; i++) {
            const part = parts[i].trim();
            if (i === 0) {
                // locate
                if (part.startsWith('class.')) {
                    nodes = byClass(nodes, part.slice(6));
                } else if (part.startsWith('tag.')) {
                    nodes = byTag(nodes, part.slice(4));
                } else if (part.startsWith('.') && part.length > 1) {
                    nodes = byClass(nodes, part.slice(1));
                } else if (part.startsWith('id.')) {
                    nodes = byId(nodes, part.slice(3));
                } else if (part.startsWith('css:')) {
                    // treat as tag
                    const sel = part.slice(4).trim();
                    nodes = byTag(nodes, sel.replace(/[.#][\w-]+/g, '').split(' ')[0] || 'div');
                } else if (part.startsWith('$')) {
                    return '';
                } else {
                    nodes = byTag(nodes, part);
                }
            } else {
                // extractor
                if (part === 'text') {
                    out = nodes.map(stripTags).join(' ').trim();
                } else if (part === 'html') {
                    out = nodes.join('');
                } else if (part === 'href') {
                    const m = nodes.join('').match(/href=["']([^"']+)["']/i);
                    out = m ? m[1] : '';
                } else if (part === 'src') {
                    const m = nodes.join('').match(/src=["']([^"']+)["']/i);
                    out = m ? m[1] : '';
                } else if (part === 'textNodes') {
                    out = nodes.map(n => (n.match(/>([^<>]+)</g) || []).map(x => x.slice(1, -1)).join(' ')).join(' ').trim();
                }
            }
        }
        return out;
    } catch (e) {
        return '';
    }
}

function stripTags(s) {
    return String(s).replace(/<[^>]*>/g, ' ').replace(/\s+/g, ' ').trim();
}

function byClass(nodes, cls) {
    const re = new RegExp('<[^>]+class=["\'][^"\']*\\b' + cls.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '\\b[^"\']*["\'][^>]*>');
    const out = [];
    for (const n of nodes) {
        let rest = n;
        let m;
        while ((m = re.exec(rest))) {
            const tagMatch = m[0].match(/^<([a-zA-Z0-9]+)/);
            const tag = tagMatch ? tagMatch[1] : 'div';
            const open = m[0];
            const closeRe = new RegExp('</' + tag + '>');
            const close = rest.match(closeRe);
            if (close) {
                out.push(rest.slice(m.index, close.index + close[0].length));
                rest = rest.slice(close.index + close[0].length);
            } else {
                rest = rest.slice(m.index + open.length);
            }
        }
    }
    return out;
}

function byTag(nodes, tag) {
    const openRe = new RegExp('<' + tag + '\\b[^>]*>', 'gi');
    const out = [];
    for (const n of nodes) {
        let rest = n;
        let m;
        while ((m = openRe.exec(rest))) {
            const closeRe = new RegExp('</' + tag + '>', 'i');
            const close = rest.match(closeRe);
            if (close) {
                out.push(rest.slice(m.index, close.index + close[0].length));
                rest = rest.slice(close.index + close[0].length);
            } else {
                rest = rest.slice(m.index + m[0].length);
            }
        }
    }
    return out;
}

function byId(nodes, id) {
    const re = new RegExp('<[^>]+id=["\']' + id.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '["\'][^>]*>');
    const out = [];
    for (const n of nodes) {
        let rest = n;
        let m;
        while ((m = re.exec(rest))) {
            const tagMatch = m[0].match(/^<([a-zA-Z0-9]+)/);
            const tag = tagMatch ? tagMatch[1] : 'div';
            const closeRe = new RegExp('</' + tag + '>');
            const close = rest.match(closeRe);
            if (close) {
                out.push(rest.slice(m.index, close.index + close[0].length));
                rest = rest.slice(close.index + close[0].length);
            } else {
                rest = rest.slice(m.index + m[0].length);
            }
        }
    }
    return out;
}

// ── cookie.* API ──
globalThis.cookie = {
    _jar: {},
    removeCookie: function(key) {
        delete this._jar[String(key)];
    }
};

// ── source.* API ──
globalThis.source = {
    getKey: function() {
        return globalThis.__SOURCE_KEY__ || '';
    },
    getLoginHeader: function() {
        return globalThis.__LOGIN_HEADER__ || '';
    },
    getBookSourceComment: function() {
        return globalThis.__SOURCE_COMMENT__ || '';
    }
};

// ── cache.* API ──
globalThis.cache = {
    _data: {},
    get: function(key) {
        return this._data[String(key)];
    },
    put: function(key, value, ttl) {
        this._data[String(key)] = value;
    }
};

// ── org.jsoup subset ──
// `Jsoup.parse(html).select('css').attr('name')` — the pattern used by many
// Legado @js sources to pull a hidden form field (token, action) before a POST.
// Supports a useful CSS subset: tag, .class, #id, tag[attr=val], [attr],
// descendant (space) selectors and `:eq(n)`.

function jsoupParse(html) {
    const doc = {
        _nodes: [String(html)],
        select: function(sel) { return new Elements(findNodes(this._nodes, sel)); },
        text: function() { return stripTags(String(html)).trim(); },
        title: function() {
            const m = String(html).match(/<title[^>]*>([\s\S]*?)<\/title>/i);
            return m ? stripTags(m[1]).trim() : '';
        },
        // jsoup Document.select is also exposed directly
        attr: function() { return ''; },
        first: function() { return this; },
    };
    return doc;
}

function Elements(nodes) {
    const el = {
        _nodes: nodes,
        length: nodes.length,
        attr: function(name) {
            for (const n of nodes) {
                const v = attrOf(n, name);
                if (v !== null && v !== undefined && v !== '') return v;
            }
            return '';
        },
        text: function() { return nodes.map(stripTags).join(' ').trim(); },
        first: function() { return new Elements(nodes.slice(0, 1)); },
        eq: function(i) { return new Elements(nodes.slice(i, i + 1)); },
        select: function(sel) { return new Elements(findNodes(nodes, sel)); },
        get: function(i) { return nodes[i]; },
        size: function() { return nodes.length; },
        empty: function() { return nodes.length === 0; },
        val: function() { return this.attr('value'); },
    };
    nodes.forEach((n, i) => { el[i] = n; });
    return el;
}

// Find element substrings matching a single CSS selector part within nodes.
function findNodes(nodes, selector) {
    let cur = nodes;
    for (const part of selector.trim().split(/\s+/).filter(Boolean)) {
        cur = applySelector(cur, part);
    }
    return cur;
}

function applySelector(nodes, part) {
    // Strip pseudo-selectors we can't handle except :eq(n)
    let eqIdx = null;
    let eq = part.match(/:eq\((\d+)\)/);
    if (eq) { eqIdx = parseInt(eq[1], 10); part = part.replace(/:eq\(\d+\)/, ''); }
    let out = [];
    for (const n of nodes) {
        let matches = matchSelectorPart(n, part);
        out = out.concat(matches);
    }
    if (eqIdx !== null && eqIdx >= 0) out = out.slice(eqIdx, eqIdx + 1);
    return out;
}

function matchSelectorPart(node, part) {
    if (part === '' || part === '*') return extractAll(node);
    // tag[attr=val] / tag[attr] / tag#id / tag.class / tag
    let rest = part;
    let tag = '*';
    const tm = rest.match(/^[a-zA-Z0-9]+/);
    if (tm) { tag = tm[0]; rest = rest.slice(tm[0].length); }

    let attrName = null, attrVal = null, attrOp = null;
    const am = rest.match(/\[([^\]=~^$*]+)\s*([~^$*]?=)\s*"([^"]*)"\]/);
    if (am) { attrName = am[1]; attrOp = am[2]; attrVal = am[3]; rest = rest.replace(am[0], ''); }
    else {
        const am2 = rest.match(/\[([^\]=~^$*]+)\s*([~^$*]?=)\s*'([^']*)'\]/);
        if (am2) { attrName = am2[1]; attrOp = am2[2]; attrVal = am2[3]; rest = rest.replace(am2[0], ''); }
        else {
            const am3 = rest.match(/\[([^\]=~^$*]+)\s*([~^$*]?=)\s*([^\]]+)\]/);
            if (am3) { attrName = am3[1]; attrOp = am3[2] || '='; attrVal = am3[3].trim().replace(/^["']|["']$/g, ''); rest = rest.replace(am3[0], ''); }
            else {
                const am4 = rest.match(/\[([^\]=~^$*]+)\]/);
                if (am4) { attrName = am4[1]; rest = rest.replace(am4[0], ''); }
            }
        }
    }

    let cls = null, id = null;
    const cm = rest.match(/\.([\w-]+)/g);
    if (cm) { cls = cm.map(c => c.slice(1)); rest = rest.replace(/\.[\w-]+/g, ''); }
    const im = rest.match(/#([\w-]+)/);
    if (im) { id = im[1]; rest = rest.replace(/#[\w-]+/, ''); }

    const els = tag === '*' ? extractAll(node) : extractByTag(node, tag);
    return els.filter(el => {
        if (cls) for (const c of cls) if (!hasClass(el, c)) return false;
        if (id) { const v = attrOf(el, 'id'); if (v !== id) return false; }
        if (attrName) {
            const v = attrOf(el, attrName);
            if (v === null || v === undefined) return false;
            if (attrVal !== null && !attrMatch(v, attrVal, attrOp || '=')) return false;
        }
        return true;
    });
}

function attrMatch(value, want, op) {
    switch (op) {
        case '=': return value === want;
        case '~=': return value.split(/\s+/).includes(want);
        case '^=': return value.startsWith(want);
        case '$=': return value.endsWith(want);
        case '*=': return value.includes(want);
        default: return false;
    }
}

// Extract the value of an attribute from an element substring.
function attrOf(el, name) {
    const m = el.match(new RegExp('<[^>]*\\b' + name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '\\s*=\\s*("([^"]*)"|\'([^\']*)\'|[^\\s>]+)'), 'i');
    if (!m) return null;
    return m[2] !== undefined ? m[2] : (m[3] !== undefined ? m[3] : m[1]);
}

function hasClass(el, cls) {
    const v = attrOf(el, 'class');
    return v !== null && v.split(/\s+/).includes(cls);
}

// Find all elements of `tag` (with nesting) in an HTML string, returned as
// element substrings (from `<tag` to its matching `</tag>`).
function extractByTag(html, tag) {
    const re = new RegExp('<' + tag + '\\b[^>]*>', 'gi');
    const out = [];
    for (const src of [html]) {
        let rest = src;
        let m;
        while ((m = re.exec(rest))) {
            const openEnd = rest.indexOf('>', m.index + m[0].length);
            const start = m.index;
            // If self-closing, no close tag
            const openTag = rest.slice(start, openEnd >= 0 ? openEnd + 1 : rest.length);
            if (/\/\s*>$/.test(openTag)) {
                out.push(openTag);
                rest = rest.slice(start + openTag.length);
                re.lastIndex = 0;
                continue;
            }
            // Walk nesting to find matching close
            const closeRe = new RegExp('</' + tag + '\\s*>', 'i');
            let depth = 1;
            let pos = start + openTag.length;
            let end = -1;
            const innerRe = new RegExp('<' + tag + '\\b|</' + tag + '\\s*>', 'gi');
            let inner;
            const slice = rest.slice(pos);
            while ((inner = innerRe.exec(slice))) {
                if (inner[0].startsWith('</')) depth--;
                else depth++;
                if (depth === 0) {
                    end = pos + inner.index + inner[0].length;
                    break;
                }
            }
            if (end < 0) end = rest.length;
            out.push(rest.slice(start, end));
            rest = rest.slice(end);
            re.lastIndex = 0;
        }
    }
    return out;
}

// Extract every top-level element substring from an HTML string.
function extractAll(html) {
    const out = [];
    const re = /<([a-zA-Z0-9]+)\b[^>]*>/g;
    let rest = html;
    let m;
    while ((m = re.exec(rest))) {
        const tag = m[1];
        const els = extractByTag(rest.slice(m.index), tag);
        if (els.length) {
            out.push(els[0]);
            const consumed = rest.indexOf(els[0], m.index);
            const next = consumed + els[0].length;
            rest = rest.slice(next);
            re.lastIndex = 0;
        } else {
            rest = rest.slice(m.index + m[0].length);
            re.lastIndex = 0;
        }
    }
    return out;
}

// ── Java built-in objects ──
globalThis.Packages = {
    org: {
        jsoup: {
            Jsoup: {
                parse: jsoupParse
            },
            nodes: {
                Element: function() {},
                Document: function() {},
                Elements: Elements
            },
            select: {
                Elements: Elements
            }
        }
    },
    java: {
        lang: {
            String: String,
            System: { currentTimeMillis: () => Date.now() }
        },
        text: { SimpleDateFormat: function() { return { format: (d) => String(d) }; } },
        net: { URLEncoder: { encode: (s) => encodeURIComponent(s) } }
    },
    // Namespaces used only as importPackage() targets; the classes themselves
    // are exposed by the JavaImporter polyfill.
    javax: {
        crypto: {
            spec: {},
            Cipher: {}
        }
    },
    android: {
        util: { Base64: {} }
    }
};

// Sources reference `org.jsoup.Jsoup.parse(...)` directly (not via Packages).
globalThis.org = globalThis.Packages.org;
globalThis.jsoup = globalThis.Packages.org.jsoup;

// `with(javaImport)` exposes crypto classes to rules that use JavaImporter.
globalThis.JavaImporter = function() {
    const imp = {};
    imp.importPackage = function() {
        // JavaImporter.importPackage brings java.* / javax.* classes into scope.
        // We expose the classes we polyfill so `with(javaImport){ SecretKeySpec... }`
        // rules resolve them.
        imp.SecretKeySpec = globalThis.SecretKeySpec;
        imp.IvParameterSpec = globalThis.IvParameterSpec;
        imp.Base64 = globalThis.Base64;
        imp.Cipher = globalThis.Cipher;
        imp.Arrays = globalThis.Arrays;
        imp.String = globalThis.String;
        imp.Base64 = globalThis.Base64;
        return imp;
    };
    imp.importClass = imp.importPackage;
    return imp;
};

// ── Java crypto classes (SecretKeySpec / IvParameterSpec / Base64 / Cipher) ──
// Sources that decrypt paginated content use the classic Java API:
//   key = SecretKeySpec(String("...").getBytes(), "DESede");
//   iv  = IvParameterSpec(String("...").getBytes());
//   bytes = Base64.decode(String(str).getBytes(), 2);
//   chipher = Cipher.getInstance("DESede/CBC/PKCS5Padding");
//   chipher.init(2, key, iv);
//   return String(chipher.doFinal(bytes));
// Polyfilling these lets `content_check` actually verify such sources.

// String.getBytes() — legacy Java call used by crypto sources.
if (typeof String.prototype.getBytes !== 'function') {
    String.prototype.getBytes = function() {
        const s = String(this);
        const arr = new Uint8Array(s.length);
        for (let i = 0; i < s.length; i++) arr[i] = s.charCodeAt(i) & 0xff;
        return arr;
    };
}

const __byteArrayOf = function(v) {
    if (typeof v === 'string') {
        const s = String(v);
        const arr = new Uint8Array(s.length);
        for (let i = 0; i < s.length; i++) arr[i] = s.charCodeAt(i) & 0xff;
        return arr;
    }
    if (v instanceof Uint8Array || Array.isArray(v)) return new Uint8Array(v);
    if (v instanceof Buffer) return new Uint8Array(v);
    return new Uint8Array(0);
};
globalThis.SecretKeySpec = function(key, algorithm) {
    if (!(this instanceof SecretKeySpec)) return new SecretKeySpec(key, algorithm);
    this.key = __byteArrayOf(key);
    this.algorithm = String(algorithm || 'AES');
};

globalThis.IvParameterSpec = function(iv) {
    if (!(this instanceof IvParameterSpec)) return new IvParameterSpec(iv);
    this.iv = __byteArrayOf(iv);
};

// `with(javaImport)` exposes crypto classes to rules that use JavaImporter.
globalThis.javaImport = {};

// java.util.Base64.decode(byte[], flags)
globalThis.Base64 = {
    decode: function(input, flags) {
        const bytes = __byteArrayOf(input);
        // input is the ASCII bytes of a base64 string — decode in one step.
        const buf = Buffer.from(bytes);
        return Buffer.from(buf.toString('utf8').trim(), 'base64');
    }
};

// Cipher.getInstance("AES/CBC/PKCS5Padding"); init(2=DECRYPT, key, iv); doFinal(bytes)
globalThis.Cipher = {
    getInstance: function(transform) {
        const t = String(transform || '');
        const cipher = {};
        let keySpec = null;
        let ivSpec = null;
        let mode = 2;
        cipher.init = function(m, key, iv) {
            mode = m;
            keySpec = key && key.key ? key : null;
            ivSpec = iv && iv.iv ? iv : null;
        };
        cipher.doFinal = function(bytes) {
            if (!keySpec) return Buffer.alloc(0);
            try {
                const crypto = require('crypto');
                const keyBuf = Buffer.from(keySpec.key);
                const ivBuf = ivSpec ? Buffer.from(ivSpec.iv) : null;
                const dataBuf = Buffer.from(__byteArrayOf(bytes));
                // DESede: 24-byte key / 8-byte iv. DES: 8-byte key / 8-byte iv.
                if (/DES(?:ede|3)/i.test(t)) {
                    const algo = ivBuf ? 'des-ede3-cbc' : 'des-ede3';
                    const algoName = keyBuf.length === 24 ? algo : 'des-ede3-cbc';
                    const dec = crypto.createDecipheriv(algoName, keyBuf, ivBuf || Buffer.alloc(8));
                    return Buffer.concat([dec.update(dataBuf), dec.final()]);
                }
                if (/DES\//i.test(t) && !/DESede|DES3/i.test(t)) {
                    const dec = crypto.createDecipheriv('des-cbc', keyBuf, ivBuf || Buffer.alloc(8));
                    return Buffer.concat([dec.update(dataBuf), dec.final()]);
                }
                const algo = keyBuf.length === 32 ? 'aes-256-cbc' : (keyBuf.length === 24 ? 'aes-192-cbc' : 'aes-128-cbc');
                const dec = crypto.createDecipheriv(algo, keyBuf, ivBuf || Buffer.alloc(16));
                return Buffer.concat([dec.update(dataBuf), dec.final()]);
            } catch (e) {
                return Buffer.alloc(0);
            }
        };
        return cipher;
    }
};

globalThis.javaImport.SecretKeySpec = globalThis.SecretKeySpec;
globalThis.javaImport.IvParameterSpec = globalThis.IvParameterSpec;
globalThis.javaImport.Base64 = globalThis.Base64;
globalThis.javaImport.Cipher = globalThis.Cipher;
globalThis.Arrays = {
    copyOfRange: function(src, from, to) {
        const arr = __byteArrayOf(src);
        return Buffer.from(arr.slice(from, to));
    }
};
globalThis.javaImport.Arrays = globalThis.Arrays;

// Result extraction is handled inline in runner.rs

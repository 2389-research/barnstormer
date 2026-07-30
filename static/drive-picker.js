// ABOUTME: Google Drive Picker integration for the launchpad composer.
// ABOUTME: Lazy-loads Google APIs on first use; picked files become composer chips.
//
// NOTE: untested in CI — requires a real Web OAuth client + API key (set via
// GOOGLE_DRIVE_CLIENT_ID / GOOGLE_DRIVE_API_KEY) and a browser OAuth consent.
// The button only renders when those are configured (see launchpad.html).

(function () {
    "use strict";

    var GAPI_SRC = "https://apis.google.com/js/api.js";
    var GSI_SRC = "https://accounts.google.com/gsi/client";
    // drive.file: app only sees files the user explicitly picks — minimal scope.
    var SCOPE = "https://www.googleapis.com/auth/drive.file";

    var apisReady = false;

    function loadScript(src) {
        return new Promise(function (resolve, reject) {
            if (document.querySelector('script[src="' + src + '"]')) { resolve(); return; }
            var s = document.createElement("script");
            s.src = src;
            s.async = true;
            s.defer = true;
            s.onload = function () { resolve(); };
            s.onerror = function () { reject(new Error("failed to load " + src)); };
            document.head.appendChild(s);
        });
    }

    function ensureApis() {
        if (apisReady) return Promise.resolve();
        return Promise.all([loadScript(GAPI_SRC), loadScript(GSI_SRC)])
            .then(function () {
                return new Promise(function (resolve) {
                    window.gapi.load("picker", function () { resolve(); });
                });
            })
            .then(function () { apisReady = true; });
    }

    function getToken(clientId) {
        return new Promise(function (resolve, reject) {
            try {
                var tokenClient = window.google.accounts.oauth2.initTokenClient({
                    client_id: clientId,
                    scope: SCOPE,
                    callback: function (resp) {
                        if (resp && resp.access_token) resolve(resp.access_token);
                        else reject(new Error("no access token"));
                    },
                });
                tokenClient.requestAccessToken({ prompt: "" });
            } catch (e) { reject(e); }
        });
    }

    function showPicker(token, apiKey) {
        return new Promise(function (resolve) {
            var g = window.google;
            var view = new g.picker.DocsView(g.picker.ViewId.DOCS)
                .setIncludeFolders(true)
                .setSelectFolderEnabled(false);
            var picker = new g.picker.PickerBuilder()
                .enableFeature(g.picker.Feature.MULTISELECT_ENABLED)
                .setOAuthToken(token)
                .setDeveloperKey(apiKey)
                .addView(view)
                .setCallback(function (data) {
                    if (data.action === g.picker.Action.PICKED) resolve(data.docs || []);
                    else if (data.action === g.picker.Action.CANCEL) resolve([]);
                })
                .build();
            picker.setVisible(true);
        });
    }

    // Native Google formats can't be downloaded raw — export them; everything
    // else downloads via alt=media.
    var EXPORT = {
        "application/vnd.google-apps.document": "application/pdf",
        "application/vnd.google-apps.spreadsheet": "text/csv",
        "application/vnd.google-apps.presentation": "application/pdf",
        "application/vnd.google-apps.drawing": "image/png",
    };
    var EXT = { "application/pdf": "pdf", "text/csv": "csv", "image/png": "png" };

    function fetchFile(doc, token) {
        var id = doc.id;
        var name = doc.name || "drive-file";
        var mime = doc.mimeType || "";
        var url, outMime;
        if (EXPORT[mime]) {
            outMime = EXPORT[mime];
            url = "https://www.googleapis.com/drive/v3/files/" + id +
                "/export?mimeType=" + encodeURIComponent(outMime);
            if (EXT[outMime] && !/\.[a-z0-9]+$/i.test(name)) name += "." + EXT[outMime];
        } else {
            outMime = mime || "application/octet-stream";
            url = "https://www.googleapis.com/drive/v3/files/" + id + "?alt=media";
        }
        return fetch(url, { headers: { Authorization: "Bearer " + token } })
            .then(function (r) {
                if (!r.ok) throw new Error("drive download failed: " + r.status);
                return r.blob();
            })
            .then(function (blob) { return new File([blob], name, { type: outMime }); });
    }

    function onClick(btn) {
        var clientId = btn.getAttribute("data-client-id");
        var apiKey = btn.getAttribute("data-api-key");
        var form = document.querySelector("#launchpad-form");
        btn.disabled = true;
        ensureApis()
            .then(function () { return getToken(clientId); })
            .then(function (token) {
                return showPicker(token, apiKey).then(function (docs) {
                    return Promise.all(docs.map(function (d) { return fetchFile(d, token); }));
                });
            })
            .then(function (files) {
                if (files.length && form) {
                    form.dispatchEvent(new CustomEvent("composer:add-files", { detail: { files: files } }));
                }
            })
            .catch(function (e) { console.error("Drive picker error:", e); })
            .finally(function () { btn.disabled = false; });
    }

    function bind(root) {
        if (!root || !root.querySelectorAll) return;
        if (root.matches && root.matches("[data-drive-picker]")) bindOne(root);
        root.querySelectorAll("[data-drive-picker]").forEach(bindOne);
    }
    function bindOne(b) {
        if (b.dataset.drivePickerBound) return;
        b.dataset.drivePickerBound = "1";
        b.addEventListener("click", function () { onClick(b); });
    }

    document.addEventListener("DOMContentLoaded", function () { bind(document); });
    document.body.addEventListener("htmx:afterSwap", function (e) { bind(e.target); });
})();

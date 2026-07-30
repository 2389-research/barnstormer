// ABOUTME: Composer attach UX — file chips, drag-and-drop, and clipboard paste.
// ABOUTME: Drives the launchpad composer (hold + chips) and the in-spec context dropzone.

(function () {
    "use strict";

    function fmtSize(n) {
        if (n < 1024) return n + " B";
        if (n < 1048576) return Math.round(n / 1024) + " KB";
        return (n / 1048576).toFixed(1) + " MB";
    }

    var IMG = ["png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "svg"];
    var CODE = ["rs", "py", "js", "ts", "go", "rb", "java", "c", "cpp", "h", "sh", "html", "css", "json", "yaml", "yml", "toml"];

    function badgeFor(name) {
        var ext = (name.split(".").pop() || "").toLowerCase();
        if (IMG.indexOf(ext) !== -1) return { cls: "chip-ic-img", label: ext };
        if (ext === "pdf") return { cls: "chip-ic-pdf", label: "pdf" };
        if (CODE.indexOf(ext) !== -1) return { cls: "chip-ic-code", label: ext };
        return { cls: "chip-ic-note", label: ext || "file" };
    }

    // --- Launchpad composer: hold files as removable chips until submit ---
    function initComposer(form) {
        if (!form || form.dataset.composerInit) return;
        var input = form.querySelector(".composer-file-input");
        var chips = form.querySelector(".composer-chips");
        if (!input || !chips) return;
        form.dataset.composerInit = "1";

        var dt = new DataTransfer();

        function render() {
            chips.innerHTML = "";
            Array.prototype.forEach.call(dt.files, function (f, i) {
                var b = badgeFor(f.name);
                var chip = document.createElement("div");
                chip.className = "chip";
                chip.innerHTML =
                    '<span class="chip-ic ' + b.cls + '">' + b.label + "</span>" +
                    '<span class="chip-meta"><span class="chip-name"></span>' +
                    '<span class="chip-sub">' + fmtSize(f.size) + "</span></span>" +
                    '<button type="button" class="chip-x" aria-label="Remove" data-index="' + i + '">&times;</button>";
                chip.querySelector(".chip-name").textContent = f.name;
                chips.appendChild(chip);
            });
            input.files = dt.files; // keep the real input in sync so submit carries them
        }

        function add(fileList) {
            Array.prototype.forEach.call(fileList, function (f) { dt.items.add(f); });
            render();
        }

        // External sources (e.g. the Google Drive picker) inject files as chips.
        form.addEventListener("composer:add-files", function (e) {
            if (e.detail && e.detail.files && e.detail.files.length) add(e.detail.files);
        });

        input.addEventListener("change", function () {
            // The native picker replaces selection; merge into our held set.
            add(input.files);
        });

        ["dragenter", "dragover"].forEach(function (ev) {
            form.addEventListener(ev, function (e) { e.preventDefault(); form.classList.add("drag"); });
        });
        form.addEventListener("dragleave", function (e) {
            if (!form.contains(e.relatedTarget)) form.classList.remove("drag");
        });
        form.addEventListener("drop", function (e) {
            e.preventDefault();
            form.classList.remove("drag");
            if (e.dataTransfer && e.dataTransfer.files.length) add(e.dataTransfer.files);
        });

        var ta = form.querySelector("textarea");
        if (ta) ta.addEventListener("paste", function (e) {
            var files = e.clipboardData && e.clipboardData.files;
            if (files && files.length) add(files);
        });

        chips.addEventListener("click", function (e) {
            var btn = e.target.closest(".chip-x");
            if (!btn) return;
            var idx = parseInt(btn.getAttribute("data-index"), 10);
            var next = new DataTransfer();
            Array.prototype.forEach.call(dt.files, function (f, i) { if (i !== idx) next.items.add(f); });
            dt = next;
            render();
        });
    }

    // --- In-spec context panel: drop/paste uploads immediately (panel re-renders) ---
    function initContextDropzone(panel) {
        if (!panel || panel.dataset.dropInit) return;
        var form = panel.querySelector("#context-upload-form");
        var input = panel.querySelector(".context-file-input");
        if (!form || !input) return;
        panel.dataset.dropInit = "1";

        function submitWith(fileList) {
            var dt = new DataTransfer();
            Array.prototype.forEach.call(fileList, function (f) { dt.items.add(f); });
            input.files = dt.files;
            form.requestSubmit();
        }

        ["dragenter", "dragover"].forEach(function (ev) {
            panel.addEventListener(ev, function (e) { e.preventDefault(); panel.classList.add("drag"); });
        });
        panel.addEventListener("dragleave", function (e) {
            if (!panel.contains(e.relatedTarget)) panel.classList.remove("drag");
        });
        panel.addEventListener("drop", function (e) {
            e.preventDefault();
            panel.classList.remove("drag");
            if (e.dataTransfer && e.dataTransfer.files.length) submitWith(e.dataTransfer.files);
        });
        panel.addEventListener("paste", function (e) {
            var files = e.clipboardData && e.clipboardData.files;
            if (files && files.length) submitWith(files);
        });
    }

    function scan(root) {
        if (!root || !root.querySelectorAll) return;
        if (root.matches && root.matches("#launchpad-form")) initComposer(root);
        root.querySelectorAll("#launchpad-form").forEach(initComposer);
        if (root.matches && root.matches("#context-panel")) initContextDropzone(root);
        root.querySelectorAll("#context-panel").forEach(initContextDropzone);
    }

    document.addEventListener("DOMContentLoaded", function () { scan(document); });
    // Re-init when HTMX swaps the launchpad or context panel into the page.
    document.body.addEventListener("htmx:afterSwap", function (e) { scan(e.target); });
})();

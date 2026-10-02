// Ocre direct uploads (Active Storage's `direct_upload: true`) and rich text
// embeds (Action Text attachments), served at
// /ocre/direct-upload.js by `ocre::storage::direct_upload_script()`.
//
// <input type="file" name="video" data-direct-upload-url="/videos/uploads">
//
// When its form is submitted, each chosen file goes straight from the browser
// to R2: a POST to the input's URL (`storage::direct_upload` answers the signed
// PUT), then the PUT itself. The form is then submitted without the file,
// with hidden fields `video_key` (the signed key) and `video_filename` for
// `storage::attach_direct_upload`. With `multiple`, the fields repeat.
//
// <input type="file" name="video" data-multipart-upload-url="/videos/uploads">
//
// Large files (`storage::multipart_uploads`): the file is sent in parts, 4 at
// a time, each retried 3 times. The finished parts are kept in localStorage,
// so after a lost connection or a closed tab, choosing the same file again
// sends only the missing parts. The form then gets the same hidden fields.
//
// Events, dispatched on the input and bubbling (Active Storage's names):
// direct-uploads:start / direct-uploads:end on the form, and per file
// direct-upload:start, direct-upload:progress (detail.progress, 0 to 100),
// direct-upload:error (detail.error; call preventDefault() to skip the alert)
// and direct-upload:end, each with detail.file.
(() => {
  const emit = (target, name, detail = {}) =>
    target.dispatchEvent(new CustomEvent(name, { bubbles: true, cancelable: true, detail }));

  const put = (upload, file, input) =>
    new Promise((resolve, reject) => {
      const request = new XMLHttpRequest();
      request.open("PUT", upload.url);
      for (const [name, value] of Object.entries(upload.headers)) request.setRequestHeader(name, value);
      request.upload.addEventListener("progress", (event) => {
        if (event.lengthComputable) emit(input, "direct-upload:progress", { file, progress: (event.loaded / event.total) * 100 });
      });
      request.addEventListener("load", () =>
        request.status >= 200 && request.status < 300 ? resolve() : reject(new Error(`R2 answered ${request.status}`)),
      );
      request.addEventListener("error", () => reject(new Error("the upload failed")));
      request.send(file);
    });

  const sign = async (input, file) => {
    const response = await fetch(input.dataset.directUploadUrl, {
      method: "POST",
      headers: { "content-type": "application/json", accept: "application/json" },
      body: JSON.stringify({ filename: file.name, content_type: file.type || "application/octet-stream", size: file.size }),
    });
    const body = await response.json().catch(() => ({}));
    if (!response.ok) {
      const fields = body.error && body.error.fields ? Object.values(body.error.fields).flat().join(", ") : "";
      throw new Error(fields || (body.error && body.error.message) || `the app answered ${response.status}`);
    }
    return body;
  };

  const json = async (url, body) => {
    const response = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json", accept: "application/json" },
      body: JSON.stringify(body),
    });
    const answer = await response.json().catch(() => ({}));
    if (!response.ok) {
      const fields = answer.error && answer.error.fields ? Object.values(answer.error.fields).flat().join(", ") : "";
      throw new Error(fields || (answer.error && answer.error.message) || `the app answered ${response.status}`);
    }
    return answer;
  };

  // PUTs one part and resolves with its ETag; `onprogress(bytes)` reports the bytes sent.
  const putPart = (url, blob, onprogress) =>
    new Promise((resolve, reject) => {
      const request = new XMLHttpRequest();
      request.open("PUT", url);
      request.upload.addEventListener("progress", (event) => onprogress(event.loaded));
      request.addEventListener("load", () => {
        if (request.status < 200 || request.status >= 300) {
          return reject(Object.assign(new Error(`the part was refused (${request.status})`), { status: request.status }));
        }
        // Straight to R2: the ETag header (the bucket's CORS rule must expose it); through the Worker: JSON.
        const etag = request.getResponseHeader("etag") || (JSON.parse(request.responseText || "{}").etag ?? "");
        etag ? resolve(etag) : reject(new Error("R2 did not expose the part's ETag: add ETag to the bucket's CORS ExposeHeaders"));
      });
      request.addEventListener("error", () => reject(new Error("the upload failed")));
      request.send(blob);
    });

  const retried = async (attempt, tries = 3) => {
    for (let i = 1; ; i++) {
      try {
        return await attempt();
      } catch (error) {
        if (i >= tries) throw error;
        await new Promise((done) => setTimeout(done, 1000 * 2 ** i));
      }
    }
  };

  // Sends `file` in parts and resolves with its signed key, resuming a
  // previous attempt at the same file (same URL, name, size and date).
  const multipart = async (input, file) => {
    const url = input.dataset.multipartUploadUrl;
    const memory = `ocre-multipart:${url}:${file.name}:${file.size}:${file.lastModified}`;
    const resumed = localStorage.getItem(memory) !== null;
    try {
      return await sendParts(input, file, url, memory);
    } catch (error) {
      // The upload R2 kept is gone (completed, aborted or expired): start over once.
      if (!resumed || error.status !== 404) throw error;
      localStorage.removeItem(memory);
      return sendParts(input, file, url, memory);
    }
  };

  const sendParts = async (input, file, url, memory) => {
    let state = JSON.parse(localStorage.getItem(memory) || "null");
    if (!state) {
      const upload = await json(url, { filename: file.name, content_type: file.type || "application/octet-stream", size: file.size });
      state = { ...upload, done: {} };
      localStorage.setItem(memory, JSON.stringify(state));
    }
    const sent = {};
    const report = () => {
      const bytes = Object.values(sent).reduce((sum, value) => sum + value, 0);
      emit(input, "direct-upload:progress", { file, progress: file.size ? (bytes / file.size) * 100 : 100 });
    };
    const missing = [];
    for (let part = 1; part <= state.part_count; part++) {
      if (state.done[part]) sent[part] = Math.min(state.part_size, file.size - (part - 1) * state.part_size);
      else missing.push(part);
    }
    report();
    const ids = { signed_key: state.signed_key, upload_id: state.upload_id };
    for (let first = 0; first < missing.length; first += 1000) {
      const { urls } = await json(`${url}/parts`, { ...ids, parts: missing.slice(first, first + 1000) });
      const queue = Object.entries(urls).map(([part, partUrl]) => [Number(part), partUrl]);
      const worker = async () => {
        while (queue.length) {
          const [part, partUrl] = queue.shift();
          const blob = file.slice((part - 1) * state.part_size, part * state.part_size);
          const etag = await retried(() =>
            putPart(partUrl, blob, (bytes) => {
              sent[part] = bytes;
              report();
            }),
          );
          sent[part] = blob.size;
          state.done[part] = etag;
          localStorage.setItem(memory, JSON.stringify(state));
        }
      };
      await Promise.all([worker(), worker(), worker(), worker()]);
    }
    const parts = Object.entries(state.done).map(([part, etag]) => ({ part_number: Number(part), etag }));
    await json(`${url}/complete`, { ...ids, parts });
    localStorage.removeItem(memory);
    return state.signed_key;
  };

  const hidden = (form, name, value) => {
    const field = document.createElement("input");
    field.type = "hidden";
    field.name = name;
    field.value = value;
    field.dataset.directUpload = "";
    form.append(field);
  };

  // Files dropped into a Trix editor with data-embeds-url="/posts/embeds"
  // (Action Text attachments): POSTed as `file`, then shown from the `url`
  // the app answers; a refused file is removed with an alert.
  document.addEventListener("trix-attachment-add", (event) => {
    const url = event.target.dataset.embedsUrl;
    const attachment = event.attachment;
    if (!url || !attachment.file) return;
    const body = new FormData();
    body.append("file", attachment.file);
    const request = new XMLHttpRequest();
    request.open("POST", url);
    request.setRequestHeader("accept", "application/json");
    request.upload.addEventListener("progress", (progress) => {
      if (progress.lengthComputable) attachment.setUploadProgress((progress.loaded / progress.total) * 100);
    });
    request.addEventListener("load", () => {
      if (request.status === 200) {
        const { url: src } = JSON.parse(request.responseText);
        attachment.setAttributes({ url: src, href: src });
      } else {
        attachment.remove();
        alert(`${attachment.file.name} could not be added (${request.status}).`);
      }
    });
    request.send(body);
  });

  document.addEventListener("submit", async (event) => {
    const form = event.target;
    const inputs = [...form.querySelectorAll("input[type=file][data-direct-upload-url], input[type=file][data-multipart-upload-url]")].filter(
      (input) => !input.disabled && input.files.length > 0,
    );
    if (inputs.length === 0) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    emit(form, "direct-uploads:start");
    form.querySelectorAll("[data-direct-upload]").forEach((field) => field.remove());
    try {
      for (const input of inputs) {
        for (const file of input.files) {
          emit(input, "direct-upload:start", { file });
          try {
            let signedKey;
            if (input.dataset.multipartUploadUrl) {
              signedKey = await multipart(input, file);
            } else {
              const upload = await sign(input, file);
              await put(upload, file, input);
              signedKey = upload.signed_key;
            }
            hidden(form, `${input.name}_key`, signedKey);
            hidden(form, `${input.name}_filename`, file.name);
            emit(input, "direct-upload:end", { file });
          } catch (error) {
            if (emit(input, "direct-upload:error", { file, error })) alert(`${file.name}: ${error.message}`);
            throw error;
          }
        }
      }
    } catch {
      form.querySelectorAll("[data-direct-upload]").forEach((field) => field.remove());
      return;
    }
    inputs.forEach((input) => (input.disabled = true));
    emit(form, "direct-uploads:end");
    HTMLFormElement.prototype.submit.call(form);
  }, true);
})();

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
    const inputs = [...form.querySelectorAll("input[type=file][data-direct-upload-url]")].filter(
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
            const upload = await sign(input, file);
            await put(upload, file, input);
            hidden(form, `${input.name}_key`, upload.signed_key);
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

window.addEventListener("load", () => {
  const qrcodeContainer = document.getElementById("qrcode-container");

  fetch("/qr.png")
    .then((response) => {
      if (!response.ok) {
        throw new Error("Failed to fetch QR code");
      }
      return response.blob();
    })
    .then((blob) => {
      const img = document.createElement("img");
      img.src = URL.createObjectURL(blob);
      qrcodeContainer.innerHTML = "";
      qrcodeContainer.appendChild(img);
    })
    .catch((error) => {
      qrcodeContainer.innerHTML = "<span>Error loading QR Code</span>";
      console.error(error);
    });
});

document.getElementById("drag_and_drop-menu").addEventListener("click", () => {
  document.getElementById("file-input").click();
});

document
  .getElementById("drag_and_drop-menu")
  .addEventListener("dragover", (e) => {
    e.preventDefault();
    document.getElementById("drag_and_drop-menu").className =
      "drag_and_drop-menu-active";
    document.getElementById("menu").style.display = "none";
    document.getElementById("menu-active").style.display = "block";
  });

document
  .getElementById("drag_and_drop-menu")
  .addEventListener("dragend", () => {
    e.preventDefault();
    document.getElementById("drag_and_drop-menu").className =
      "drag_and_drop-menu";
    document.getElementById("menu").style.display = "flex";
    document.getElementById("menu-active").style.display = "none";
  });

document
  .getElementById("drag_and_drop-menu")
  .addEventListener("drop", async function dropHandler(ev) {
    console.log("File(s) dropped");

    document.getElementById("drag_and_drop-menu").className =
      "drag_and_drop-menu";
    document.getElementById("menu").style.display = "flex";
    document.getElementById("menu-active").style.display = "none";

    const statusDiv = document.getElementById("status");

    // Prevent default behavior (Prevent file from being opened)
    ev.preventDefault();

    if (!ev.dataTransfer.items) {
      const message = document.createElement("div");
      message.textContent = "Please select files!";
      message.classList.add("status-message", "error");
      statusDiv.appendChild(message);
      return;
    }
    const files = Array.from(ev.dataTransfer.items);
    const uploadPromises = files.map((file) =>
      uploadFile(file.getAsFile(), statusDiv)
    );
    await Promise.all(uploadPromises);
  });

document
  .getElementById("upload-button")
  .addEventListener("click", async (e) => {
    e.preventDefault();
    const fileInput = document.getElementById("file-input");
    const statusDiv = document.getElementById("status");

    if (!fileInput.files.length) {
      const message = document.createElement("div");
      message.textContent = "Please select files!";
      message.classList.add("status-message", "error");
      statusDiv.appendChild(message);
      return;
    }

    const files = Array.from(fileInput.files);
    const uploadPromises = files
      .toReversed()
      .map((file) => uploadFile(file, statusDiv));
    await Promise.all(uploadPromises);
  });

async function uploadFile(file, statusDiv) {
  const formData = new FormData();
  formData.append("size", file.size);
  formData.append("file", file);

  console.log(`file size: ${file.size}`);

  const message = document.createElement("div");
  message.classList.add("idle");
  message.classList.add("status-message");
  const fileNameSpan = document.createElement("span");
  fileNameSpan.classList.add("file_name");
  const status = document.createElement("span");
  status.classList.add("status_message");
  fileNameSpan.textContent = file.name;
  message.appendChild(fileNameSpan);
  message.appendChild(status);
  statusDiv.appendChild(message);

  try {
    console.log(`Opening progress connection for `);
    const eventSource = await openProgressConnection(file);

    console.log("Connection opened");
    trackProgress(eventSource, file, message, status);

    console.log("Sending upload request..");
    const response = await fetch("/upload", {
      method: "POST",
      body: formData,
    });

    if (!response.ok) {
      const errorText = await response.text();
      console.log(errorText);
      status.textContent = `FAILURE`;
      message.className = "status-message error";
      return;
    }

    status.textContent = `SUCCESS`;
    message.className = "status-message success";
  } catch (error) {
    message.textContent = `FAILURE`;
    message.className = "status-message error";
  }
}

async function openProgressConnection(file) {
  return new Promise((resolve, reject) => {
    const eventSource = new EventSource(`/progress/${file.name}`);

    eventSource.onopen = () => {
      console.log(`Progress connection for ${file.name} established.`);
      resolve(eventSource);
    };

    eventSource.onerror = (error) => {
      console.error(
        `Error on opening progress connection for ${file.name}`,
        error
      );
      eventSource.close();
      reject(new Error(` ${file.name} FAILURE`));
    };
  });
}

async function trackProgress(eventSource, file, message, status) {
  let isComplete = false;

  eventSource.onmessage = (event) => {
    const progressData = JSON.parse(event.data);
    if (progressData.progress !== undefined) {
      const progress = progressData.progress;
      message.className = "status-message progress";

      status.textContent = ` ${progress}%`;

      if (progress === 100) {
        status.textContent = `SUCCESS`;
        message.className = "status-message success";
        isComplete = true;
        eventSource.close();
      }
    }
  };

  eventSource.onerror = (error) => {
    console.error(`Error on progress connection for ${file.name}`, error);
    if (!isComplete) {
      message.textContent = `${file.name}: Error establishing progress connection.`;
      message.classList.add("error");
      eventSource.close();
    }
  };
}

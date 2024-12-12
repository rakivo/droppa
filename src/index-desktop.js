globalFiles = [];

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
    files.forEach((file) => {
      const { message, fileNameSpan, messageStatusDiv } = createMessage(
        file.getAsFile()
      );

      const fullFileObject = {
        status: "idle",
        file: file.getAsFile(),
        message: message,
        fileNameSpan: fileNameSpan,
        messageStatusDiv: messageStatusDiv,
      };
      globalFiles.push(fullFileObject);
    });
  });

document
  .getElementById("upload-button")
  .addEventListener("click", async (e) => {
    e.preventDefault();
    const fileInput = document.getElementById("file-input");
    const statusDiv = document.getElementById("status");

    if (!globalFiles.length) {
      const message = document.createElement("div");
      message.textContent = "Please select files!";
      message.classList.add("status-message", "error");
      statusDiv.appendChild(message);
      return;
    }

    const uploadPromises = globalFiles.map((fileObject) => {
      uploadFile(fileObject);
    });
    await Promise.all(uploadPromises);
  });

document.getElementById("file-input").addEventListener("change", (e) => {
  console.log(e);
  Array.from(e.target.files).forEach((e) => {
    const { message, fileNameSpan, messageStatusDiv } = createMessage(e);

    const fullFileObject = {
      status: "idle",
      file: e,
      message: message,
      fileNameSpan: fileNameSpan,
      messageStatusDiv: messageStatusDiv,
    };

    globalFiles.push(fullFileObject);
  });
  console.log(e);
});

function createMessage(file) {
  const statusDiv = document.getElementById("status");
  const message = document.createElement("div");
  const fileNameSpan = document.createElement("span");
  const status = document.createElement("span");

  message.classList.add("idle");
  message.classList.add("status-message");
  fileNameSpan.classList.add("file_name");
  fileNameSpan.title = file.name;
  status.classList.add("status_message");

  fileNameSpan.textContent = file.name;
  message.appendChild(fileNameSpan);

  message.appendChild(status);
  statusDiv.appendChild(message);

  return {
    message: message,
    fileNameSpan: fileNameSpan,
    messageStatusDiv: status,
  };
}

async function uploadFile(fileObject) {
  if (fileObject.status == "success" || fileObject.status == "progress") {
    return;
  }

  const formData = new FormData();
  formData.append("size", fileObject.file.size);
  formData.append("file", fileObject.file);

  try {
    console.log(`Opening progress connection for `);
    const eventSource = await openProgressConnection(fileObject.file);

    console.log("Connection opened");
    trackProgress(eventSource, fileObject);

    console.log("Sending upload request..");
    const response = await fetch("/upload-desktop", {
      method: "POST",
      body: formData,
    });

    console.log(response);

    if (!response.ok) {
      const errorText = await response.text();
      console.log(errorText);
      fileObject.messageStatusDiv.textContent = `FAILURE`;
      fileObject.message.className = "status-message error";
      return;
    }

    fileObject.status = "success";
    fileObject.messageStatusDiv.textContent = `SUCCESS`;
    fileObject.message.className = "status-message success";
  } catch (error) {
    fileObject.message.textContent = `FAILURE`;
    fileObject.message.className = "status-message error";
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

async function trackProgress(eventSource, fileObject) {
  let isComplete = false;

  eventSource.onmessage = (event) => {
    console.log(event.data);
    const progressData = JSON.parse(event.data);
    if (progressData.progress !== undefined) {
      const progress = progressData.progress;
      fileObject.message.className = "status-message progress";

      fileObject.messageStatusDiv.textContent = ` ${progress}%`;

      fileObject.status = "progress";

      if (progress === 100) {
        fileObject.status = "success";
        fileObject.messageStatusDiv.textContent = `SUCCESS`;
        fileObject.message.className = "status-message success";
        isComplete = true;
        eventSource.close();
      }
    }
  };

  eventSource.onerror = (error) => {
    if (!isComplete) {
      fileObject.message.textContent = `FAILURE`;
      fileObject.message.classList.add("error");
      eventSource.close();
    }
  };
}

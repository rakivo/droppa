let globalFiles = [];

document.getElementById("drag_and_drop-menu").addEventListener("click", () => {
  document.getElementById("file-input").click();
});

document
  .getElementById("drag_and_drop-menu")
  .addEventListener("dragover", (e) => {
    e.preventDefault();
    document.getElementById("drag_and_drop-menu").className = "drag_and_drop-menu-active";
    document.getElementById("menu").style.display = "none";
    document.getElementById("menu-active").style.display = "block";
  });

document
  .getElementById("drag_and_drop-menu")
  .addEventListener("dragend", (e) => {
    e.preventDefault();
    document.getElementById("drag_and_drop-menu").className = "drag_and_drop-menu";
    document.getElementById("menu").style.display = "flex";
    document.getElementById("menu-active").style.display = "none";
  });

document
  .getElementById("drag_and_drop-menu")
  .addEventListener("drop", async function dropHandler(ev) {
    ev.preventDefault();
    const statusDiv = document.getElementById("status");

    document.getElementById("drag_and_drop-menu").className = "drag_and_drop-menu";
    document.getElementById("menu").style.display = "flex";
    document.getElementById("menu-active").style.display = "none";

    if (!ev.dataTransfer.items) {
      const message = document.createElement("div");
      message.textContent = "Please select files!";
      message.classList.add("status-message", "error");
      statusDiv.appendChild(message);
      return;
    }

    const files = Array.from(ev.dataTransfer.items);
    files.forEach((file) => {
      const { message, fileNameSpan, messageStatusDiv } = createMessage(file.getAsFile());
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
    const statusDiv = document.getElementById("status");

    if (!globalFiles.length) {
      const message = document.createElement("div");
      message.textContent = "Please select files!";
      message.classList.add("status-message", "error");
      statusDiv.appendChild(message);
      return;
    }

    await uploadFilesConcurrently(globalFiles);
  });

document.getElementById("file-input").addEventListener("change", (e) => {
  Array.from(e.target.files).forEach((file) => {
    const { message, fileNameSpan, messageStatusDiv } = createMessage(file);

    const fullFileObject = {
      status: "idle",
      file: file,
      message: message,
      fileNameSpan: fileNameSpan,
      messageStatusDiv: messageStatusDiv,
    };

    globalFiles.push(fullFileObject);
  });
});

document
  .getElementById("download-button")
  .addEventListener("click", async (e) => {
    fetch('/download-files')
      .then(response => {
        if (!response.ok) {
          throw new Error('Failed to download ZIP file');
        }
        return response.blob();  // Convert response to a Blob
      })
      .then(blob => {
        const link = document.createElement('a');
        link.href = URL.createObjectURL(blob);
        link.download = 'example.zip';
        link.click();
        URL.revokeObjectURL(link.href);
      })
      .catch(error => {
        console.error('Error downloading file:', error);
      });
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

async function uploadFilesConcurrently(files, maxConcurrent = 4) {
  const semaphore = new Array(maxConcurrent).fill(Promise.resolve());
  for (const fileObject of files) {
    const slot = semaphore.shift();
    semaphore.push(slot.then(() => uploadFile(fileObject)));
  }
  await Promise.all(semaphore);
}

async function uploadFile(fileObject) {
  if (fileObject.status === "success" || fileObject.status === "progress") {
    return;
  }

  const formData = new FormData();
  formData.append("size", fileObject.file.size);
  formData.append("file", fileObject.file);

  try {
    console.log(`Opening progress connection for ${fileObject.file.name}`);
    const eventSource = await openProgressConnection(fileObject.file);

    console.log("Connection opened");
    trackProgress(eventSource, fileObject);

    console.log("Sending upload request...");
    const response = await fetch("/upload-mobile", {
      method: "POST",
      body: formData,
    });

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

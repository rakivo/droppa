// TODO: add a number after the device type, to distinguish them.
let deviceName = getDeviceType();

let connected = new Map();

const deviceSelectorSelect = document.getElementById('deviceSelectorSelect');
const deviceSelectorConnectButton = document.getElementById('deviceSelectorConnectButton');
const deviceSelectorStatusMessage = document.getElementById('deviceSelectorStatusMessage');

// Handle device selection and enable the connect button
deviceSelectorSelect.addEventListener('change', () => {
  const selectedDevice = deviceSelectorSelect.value;
  if (selectedDevice) {
    deviceSelectorConnectButton.disabled = false;
    deviceSelectorStatusMessage.textContent = '';
  }
});

// Send the connection request
deviceSelectorConnectButton.addEventListener('click', async () => {
  const deviceName = deviceSelectorSelect.value;
  const to = 'someOtherDeviceName'; // You can replace this with the target device if needed

  try {
    const response = await fetch(`/connect?deviceName=${encodeURIComponent(deviceName)}&to=${encodeURIComponent(to)}`, {
      method: 'GET'
    });

    if (response.ok) {
      deviceSelectorStatusMessage.textContent = `Connected to ${deviceName}`;
    } else {
      deviceSelectorStatusMessage.textContent = 'Failed to connect to device.';
    }
  } catch (error) {
    console.error('Error connecting to device:', error);
    deviceSelectorStatusMessage.textContent = 'Error connecting to device.';
  }
});

let globalFiles = [];

let downloadFiles = new Map();

let eventSource = null;
let devicesEventSource = null;

function connectDevicesSSE() {
  if (devicesEventSource && devicesEventSource.readyState !== EventSource.CLOSED) {
    console.log("SSE connection already active");
    return;
  }

  console.log("Establishing SSE connection...");

  devicesEventSource = new EventSource('/connected-devices');

  devicesEventSource.onopen = () => {
    console.log("SSE connection established");
    deviceSelectorStatusMessage.textContent = '';
  };

  devicesEventSource.onmessage = (event) => {
    console.log("Received SSE message:", event.data);

    try {
      const devices = JSON.parse(event.data); // Expecting an array of devices from the server
      if (devices.length > 0) {
        devices.forEach((device) => {
          if (!connected.has(device)) {
            connected.set(device, false); // Mark as not yet added to the DOM
          }
        });

        connected.forEach((domCreated, deviceName) => {
          if (!domCreated) {
            const option = document.createElement('option');
            option.value = deviceName;
            option.textContent = deviceName;
            deviceSelectorSelect.appendChild(option);
            connected.set(deviceName, true); // Mark as added to the DOM
          }
        });
      } else {
        deviceSelectorStatusMessage.textContent = 'No connected devices found.';
      }
    } catch (error) {
      console.error("Error processing SSE message:", error);
    }
  };

  devicesEventSource.onerror = (error) => {
    console.error("SSE connection error:", error);
    deviceSelectorStatusMessage.textContent = 'Failed to load devices.';
    // Close the connection and retry after a delay
    if (devicesEventSource) {
      devicesEventSource.close();
      devicesEventSource = null;
      setTimeout(() => connectDevicesSSE(), 2500);
    }
  };
}

function connectSSE() {
  if (eventSource && eventSource.readyState !== EventSource.CLOSED) {
    console.log("SSE connection already active");
    return;
  }

  console.log("Establishing SSE connection...");

  eventSource = new EventSource("/download-files-progress-desktop");

  eventSource.onopen = () => {
    console.log("SSE connection established");
  };

  eventSource.onmessage = (event) => {
    console.log("Received SSE message:", event.data);
    if (event.data === "CONNECTION_REPLACED") {
      console.log("Connection replaced by the server.");
      eventSource.close();
      return;
    }

    const eventData = JSON.parse(event.data);

    console.log(downloadFiles);
    console.log(eventData);

    eventData.forEach((messageFile) => {
      if (!downloadFiles.has(messageFile.name)) {
        downloadFiles.set(messageFile.name, messageFile);
      }
    });

    eventData.forEach((messageFile) => {
      if (downloadFiles.has(messageFile.name)) {
        const hashFile = downloadFiles.get(messageFile.name);
        hashFile.progress = messageFile.progress;
        watchDownloadFileProgress(hashFile);
      }
    });
  };

  eventSource.onerror = (error) => {
    console.error("SSE connection error:", error);
    // Close the connection to avoid endless reconnect attempts
    if (eventSource) {
      eventSource.close();
      eventSource = null;
      setTimeout(() => connectSSE(), 2500);
    }
  };
}

window.addEventListener("beforeunload", () => {
  if (eventSource) {
    console.log("Closing SSE connection on unload");
    eventSource.close();
  }
});

window.addEventListener("load", async () => {
  connectDevicesSSE();

  let deviceName_ = encodeURIComponent(deviceName);
  await fetch(`init-device?deviceName=${deviceName_}`, {
    method: "POST"
  });

  connectSSE();

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
      const span = document.createElement("span");
      span.textContent = "QR code for your phone";
      img.src = URL.createObjectURL(blob);
      qrcodeContainer.innerHTML = "";
      qrcodeContainer.appendChild(img);
      qrcodeContainer.appendChild(span);
    })
    .catch((error) => {
      qrcodeContainer.innerHTML = "<span>Error loading QR Code</span>";
      console.error(error);
    });
});

function getDeviceType() {
  const ua = navigator.userAgent;
  return /Mobi|Android/i.test(ua) ? "MOBILE" : "DESKTOP";
}

document.getElementById("device-form").addEventListener("submit", async function (e) {
  e.preventDefault();

  const customName = document.getElementById("device-name").value.trim();
  const deviceType = getDeviceType();
  const fullDeviceName = `${deviceType}-${customName}`;
  const resultDiv = document.getElementById("device-result");
  resultDiv.textContent = `Device Name: ${fullDeviceName}`;
  resultDiv.classList.remove("device-hidden");

  let deviceName_ = encodeURIComponent(deviceName);
  await fetch(`uninit-device?deviceName=${deviceName_}`, {
    method: "POST"
  });

  deviceName = fullDeviceName;
  
  deviceName_ = encodeURIComponent(deviceName);
  await fetch(`init-device?deviceName=${deviceName_}`, {
    method: "POST"
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
    ev.preventDefault();
    const statusDiv = document.getElementById("upload_status");

    document.getElementById("drag_and_drop-menu").className =
      "drag_and_drop-menu";
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
      const { message, fileNameSpan, messageStatusDiv } = createMessage(
        file.getAsFile(),
        "upload"
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
    const statusDiv = document.getElementById("upload_status");

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
    const { message, fileNameSpan, messageStatusDiv } = createMessage(
      file,
      "upload"
    );

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

function createMessage(file, transmissionType) {
  const statusDiv = document.getElementById(`${transmissionType}_status`);
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

function watchDownloadFileProgress(downloadFileObject) {
  console.log(downloadFileObject);
  /*   if (downloadFileObject.status == "success") {
       return downloadFileObject;
       } */
  if (!downloadFileObject.domCreated) {
    const { message, fileNameSpan, messageStatusDiv } = createMessage(
      downloadFileObject,
      "download"
    );

    downloadFileObject.message = message;
    downloadFileObject.fileNameSpan = fileNameSpan;
    downloadFileObject.messageStatusDiv = messageStatusDiv;
    downloadFileObject.domCreated = true;
    downloadFileObject.status = "progress";

    downloadFileObject.message.className = "status-message progress";
  }
  downloadFileObject.messageStatusDiv.textContent = ` ${downloadFileObject.progress}%`;

  if (downloadFileObject.progress == 100) {
    downloadFileObject.status = "success";
    downloadFileObject.messageStatusDiv.textContent = `SUCCESS`;
    downloadFileObject.message.className = "status-message success";
  }
  console.log(downloadFileObject);
  return downloadFileObject;
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

    let deviceName_ = encodeURIComponent(deviceName);
    const response = await fetch(`/upload-desktop?deviceName=${deviceName_}`, {
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
    let deviceName_ = encodeURIComponent(deviceName);
    const eventSource = new EventSource(`/progress/${file.name}?deviceName=${deviceName_}`);

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

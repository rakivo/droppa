document.getElementById('upload-button').addEventListener('click', async () => {
    const fileInput = document.getElementById('file-input');
    const statusDiv = document.getElementById('status');
    statusDiv.innerHTML = '';

    if (!fileInput.files.length) {
        const message = document.createElement('div');
        message.textContent = "Please select files!";
        message.classList.add('status-message', 'error');
        statusDiv.appendChild(message);
        return;
    }

    const files = Array.from(fileInput.files);
    const uploadPromises = files.map((file) => uploadFile(file, statusDiv));
    await Promise.all(uploadPromises);
});

async function uploadFile(file, statusDiv) {
    const formData = new FormData();
    formData.append('size', file.size);
    formData.append('file', file);

    const message = document.createElement('div');
    message.textContent = `Preparing upload for ${file.name}...`;
    message.classList.add('status-message');
    statusDiv.appendChild(message);

    try {
        console.log(`Opening progress connection for ${file.name}..`);
        const eventSource = await openProgressConnection(file);

        console.log("Connection opened");
        trackProgress(eventSource, file, message);

        console.log("Sending upload request..");
        const response = await fetch('/upload', {
            method: 'POST',
            body: formData,
        });

        if (!response.ok) {
            const errorText = await response.text();
            message.textContent = `${file.name} failed: ${errorText}`;
            message.classList.add('error');
            return;
        }

        message.textContent = `${file.name} uploaded successfully!`;
        message.classList.add('success');
    } catch (error) {
        message.textContent = `${file.name} error: ${error.message}`;
        message.classList.add('error');
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
            console.error(`Error on opening progress connection for ${file.name}`, error);
            eventSource.close();
            reject(new Error(`Progress connection failed for ${file.name}`));
        };
    });
}

function trackProgress(eventSource, file, message) {
    let isComplete = false;

    eventSource.onmessage = (event) => {
        const progressData = JSON.parse(event.data);
        if (progressData.progress !== undefined) {
            const progress = progressData.progress;
            message.textContent = `${file.name}: Upload progress: ${progress}%`;

            if (progress === 100) {
                message.textContent = `${file.name} uploaded successfully!`;
                message.classList.add('success');
                isComplete = true;
                eventSource.close();
            }
        }
    };

    eventSource.onerror = (error) => {
        console.error(`Error on progress connection for ${file.name}`, error);
        if (!isComplete) {
            message.textContent = `${file.name}: Error establishing progress connection.`;
            message.classList.add('error');
            eventSource.close();
        }
    };
}

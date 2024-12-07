document.getElementById('upload-button').addEventListener('click', async () => {
    const fileInput = document.getElementById('file-input');
    const statusDiv = document.getElementById('status');
    statusDiv.innerHTML = ''; // Clear previous status messages

    if (!fileInput.files.length) {
        const message = document.createElement('div');
        message.textContent = "Please select files!";
        message.classList.add('status-message', 'error');
        statusDiv.appendChild(message);
        return;
    }

    const files = Array.from(fileInput.files);

    // Start parallel uploads
    const uploadPromises = files.map((file) => uploadFile(file, statusDiv));

    // Wait for all uploads to complete
    await Promise.all(uploadPromises);
});

async function uploadFile(file, statusDiv) {
    const formData = new FormData();
    formData.append('size', file.size);
    formData.append('file', file);

    const message = document.createElement('div');
    message.textContent = `Uploading ${file.name}...`;
    message.classList.add('status-message');
    statusDiv.appendChild(message);

    try {
        listenToProgress(file.name, message);

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
    } catch (error) {
        message.textContent = `${file.name} error: ${error.message}`;
        message.classList.add('error');
    }
}

function listenToProgress(fileName, message) {
    const eventSource = new EventSource(`/progress/${fileName}`);

    eventSource.onmessage = (event) => {
        try {
            const { progress } = JSON.parse(event.data);
            message.textContent = `Uploading ${fileName}: ${progress}%`;

            console.log(progress);

            if (progress >= 100) {
                message.textContent = `${fileName} upload complete!`;
                message.classList.add('success');
                eventSource.close();
                return;
            }
        } catch (error) {
            console.error("Failed to parse progress update:", error);
        }
    };

    eventSource.onerror = () => {
        console.error(`Error receiving progress for ${fileName}`);
        message.textContent = `${fileName} upload failed!`;
        message.classList.add('error');
        eventSource.close();
    };
}

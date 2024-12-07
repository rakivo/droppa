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
    message.textContent = `Preparing upload for ${file.name}...`;
    message.classList.add('status-message');
    statusDiv.appendChild(message);

    try {
await new Promise((resolve, reject) => {
            const eventSource = new EventSource(`/progress/${file.name}`);
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
                        eventSource.close(); // Close the connection after 100% progress
                        resolve(); // Resolve the promise
                    }
                }
            };

            eventSource.onerror = (error) => {
                console.error(`Error on progress connection for ${file.name}`, error);
                if (!isComplete) {
                    message.textContent = `${file.name}: Error establishing progress connection.`;
                    message.classList.add('error');
                    eventSource.close();
                    reject(new Error(`Progress connection failed for ${file.name}`));
                }
            };
        });

        // Proceed to upload the file
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

// Version picker for multi-version documentation
(function() {
    'use strict';

    const VERSIONS_JSON_URL = '/wassette/versions.json';
    const CURRENT_VERSION = getCurrentVersion();

    function getCurrentVersion() {
        const path = window.location.pathname;
        const match = path.match(/\/wassette\/(v[\d.]+|latest)\//);
        return match ? match[1] : 'latest';
    }

    function getRelativePath() {
        const path = window.location.pathname;
        const match = path.match(/\/wassette\/(?:v[\d.]+|latest)\/(.*)/);
        return match ? match[1] : 'index.html';
    }

    function createVersionPicker(versions) {
        const relativePath = getRelativePath();
        
        const select = document.createElement('select');
        select.className = 'version-picker';
        select.setAttribute('aria-label', 'Select documentation version');
        
        versions.forEach(version => {
            const option = document.createElement('option');
            option.value = version;
            option.textContent = version;
            if (version === CURRENT_VERSION) {
                option.selected = true;
            }
            select.appendChild(option);
        });

        select.addEventListener('change', function() {
            const newVersion = this.value;
            const newPath = `/wassette/${newVersion}/${relativePath}`;
            window.location.href = newPath;
        });

        return select;
    }

    function injectVersionPicker() {
        fetch(VERSIONS_JSON_URL)
            .then(response => {
                if (!response.ok) {
                    console.warn('Could not load versions.json');
                    return null;
                }
                return response.json();
            })
            .then(data => {
                if (!data || !data.versions || data.versions.length === 0) {
                    return;
                }

                const picker = createVersionPicker(data.versions);
                
                // Try to inject into the menu bar (right side)
                const menuBar = document.querySelector('.menu-bar .right-buttons');
                if (menuBar) {
                    const container = document.createElement('div');
                    container.className = 'version-picker-container';
                    container.appendChild(picker);
                    menuBar.insertBefore(container, menuBar.firstChild);
                }
            })
            .catch(error => {
                console.warn('Error loading versions:', error);
            });
    }

    // Wait for the DOM to be ready
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', injectVersionPicker);
    } else {
        injectVersionPicker();
    }
})();

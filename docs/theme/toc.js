// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

// On This Page Table of Contents
(function() {
    'use strict';

    function createTOC() {
        // Find the main content area
        const main = document.querySelector('main');
        if (!main) return;

        // Get all h2 and h3 headings from the main content
        const headings = main.querySelectorAll('h2, h3');
        if (headings.length === 0) return;

        // Create TOC container
        const tocContainer = document.createElement('div');
        tocContainer.className = 'toc-container';
        tocContainer.setAttribute('aria-label', 'On this page');

        // Create TOC header
        const tocHeader = document.createElement('div');
        tocHeader.className = 'toc-header';
        tocHeader.textContent = 'On This Page';
        tocContainer.appendChild(tocHeader);

        // Create TOC list
        const tocList = document.createElement('ul');
        tocList.className = 'toc-list';

        // Populate TOC with headings
        headings.forEach((heading) => {
            const level = heading.tagName.toLowerCase();
            const link = heading.querySelector('a.header');
            if (!link) return;

            const listItem = document.createElement('li');
            listItem.className = `toc-item toc-${level}`;

            const tocLink = document.createElement('a');
            tocLink.href = link.getAttribute('href');
            tocLink.textContent = heading.textContent;
            tocLink.className = 'toc-link';

            listItem.appendChild(tocLink);
            tocList.appendChild(listItem);
        });

        tocContainer.appendChild(tocList);

        // Insert TOC into the page
        const contentWrapper = document.querySelector('.content');
        if (contentWrapper) {
            contentWrapper.appendChild(tocContainer);
        }

        // Add active section highlighting on scroll
        setupScrollSpy(headings);
    }

    function setupScrollSpy(headings) {
        const tocLinks = document.querySelectorAll('.toc-link');
        if (tocLinks.length === 0) return;

        // Create an intersection observer to track which heading is in view
        const observerOptions = {
            rootMargin: '-20% 0px -80% 0px',
            threshold: 0
        };

        let currentActiveLink = null;

        const observer = new IntersectionObserver((entries) => {
            entries.forEach((entry) => {
                if (entry.isIntersecting) {
                    const id = entry.target.id;
                    const correspondingLink = document.querySelector(`.toc-link[href="#${id}"]`);
                    
                    if (correspondingLink) {
                        // Remove active class from all links
                        tocLinks.forEach(link => link.classList.remove('active'));
                        
                        // Add active class to current link
                        correspondingLink.classList.add('active');
                        currentActiveLink = correspondingLink;
                    }
                }
            });
        }, observerOptions);

        // Observe all headings
        headings.forEach((heading) => {
            observer.observe(heading);
        });

        // Set the first link as active initially
        if (tocLinks.length > 0 && !currentActiveLink) {
            tocLinks[0].classList.add('active');
        }
    }

    // Initialize TOC when DOM is ready
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', createTOC);
    } else {
        createTOC();
    }
})();

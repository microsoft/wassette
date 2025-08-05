// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

async function getCurrentTime() {
    return new Date().toISOString();
}

export const time = {
    getCurrentTime
};
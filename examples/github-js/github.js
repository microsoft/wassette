// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

import { get } from "wasi:config/store@0.2.0-draft";

// Helper function to get GitHub token
async function getGitHubToken() {
  const token = await get("GITHUB_TOKEN");
  if (token === undefined || token === null) {
    throw "GITHUB_TOKEN is not set";
  }
  // Ensure token is a string
  return String(token);
}

// Helper function to make GitHub API requests
async function githubApiRequest(endpoint, options = {}) {
  const token = await getGitHubToken();
  const baseUrl = "https://api.github.com";

  const headers = {
    "Authorization": `Bearer ${token}`,
    "Accept": "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "wasmtime-mcp-github",
    ...options.headers
  };

  const url = `${baseUrl}${endpoint}`;
  const fetchOptions = {
    ...options,
    headers
  };

  try {
    const response = await fetch(url, fetchOptions);

    if (!response.ok) {
      const errorText = await response.text();
      throw `GitHub API error (${response.status}): ${errorText}`;
    }

    // Handle empty responses (like 204 No Content)
    if (response.status === 204) {
      return JSON.stringify({ success: true });
    }

    const data = await response.json();
    return JSON.stringify(data, null, 2);
  } catch (error) {
    throw `Failed to fetch ${endpoint}: ${error}`;
  }
}

// Repository operations
export async function getRepository(owner, repo) {
  return await githubApiRequest(`/repos/${owner}/${repo}`);
}

export async function createRepository(name, description, isPrivate) {
  const body = {
    name,
    private: isPrivate
  };
  if (description) {
    body.description = description;
  }
  
  return await githubApiRequest("/user/repos", {
    method: "POST",
    body: JSON.stringify(body)
  });
}

export async function forkRepository(owner, repo) {
  return await githubApiRequest(`/repos/${owner}/${repo}/forks`, {
    method: "POST"
  });
}

export async function getFileContents(owner, repo, path, ref) {
  let endpoint = `/repos/${owner}/${repo}/contents/${path}`;
  if (ref) {
    endpoint += `?ref=${ref}`;
  }
  return await githubApiRequest(endpoint);
}

export async function createOrUpdateFile(owner, repo, path, content, message, branch) {
  const body = {
    message,
    content: btoa(content)
  };
  if (branch) {
    body.branch = branch;
  }
  
  return await githubApiRequest(`/repos/${owner}/${repo}/contents/${path}`, {
    method: "PUT",
    body: JSON.stringify(body)
  });
}

export async function deleteFile(owner, repo, path, message, branch) {
  // First get the file to get its SHA
  const fileData = JSON.parse(await getFileContents(owner, repo, path, branch));
  
  const body = {
    message,
    sha: fileData.sha
  };
  if (branch) {
    body.branch = branch;
  }
  
  return await githubApiRequest(`/repos/${owner}/${repo}/contents/${path}`, {
    method: "DELETE",
    body: JSON.stringify(body)
  });
}

export async function listBranches(owner, repo, page, perPage) {
  let endpoint = `/repos/${owner}/${repo}/branches`;
  const params = new URLSearchParams();
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function createBranch(owner, repo, branch, fromBranch) {
  // Get the SHA of the source branch
  const refEndpoint = fromBranch 
    ? `/repos/${owner}/${repo}/git/ref/heads/${fromBranch}`
    : `/repos/${owner}/${repo}/git/ref/heads/main`;
  
  const refData = JSON.parse(await githubApiRequest(refEndpoint));
  const sha = refData.object.sha;
  
  const body = {
    ref: `refs/heads/${branch}`,
    sha
  };
  
  return await githubApiRequest(`/repos/${owner}/${repo}/git/refs`, {
    method: "POST",
    body: JSON.stringify(body)
  });
}

export async function listCommits(owner, repo, sha, page, perPage) {
  let endpoint = `/repos/${owner}/${repo}/commits`;
  const params = new URLSearchParams();
  if (sha) params.append("sha", sha);
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function getCommit(owner, repo, sha) {
  return await githubApiRequest(`/repos/${owner}/${repo}/commits/${sha}`);
}

export async function listTags(owner, repo, page, perPage) {
  let endpoint = `/repos/${owner}/${repo}/tags`;
  const params = new URLSearchParams();
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function getTag(owner, repo, tag) {
  return await githubApiRequest(`/repos/${owner}/${repo}/git/refs/tags/${tag}`);
}

export async function listReleases(owner, repo, page, perPage) {
  let endpoint = `/repos/${owner}/${repo}/releases`;
  const params = new URLSearchParams();
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function getLatestRelease(owner, repo) {
  return await githubApiRequest(`/repos/${owner}/${repo}/releases/latest`);
}

export async function getReleaseByTag(owner, repo, tag) {
  return await githubApiRequest(`/repos/${owner}/${repo}/releases/tags/${tag}`);
}

export async function getRepositoryTree(owner, repo, treeSha, recursive) {
  const sha = treeSha || "HEAD";
  let endpoint = `/repos/${owner}/${repo}/git/trees/${sha}`;
  if (recursive) {
    endpoint += "?recursive=1";
  }
  return await githubApiRequest(endpoint);
}

export async function searchCode(query, page, perPage) {
  let endpoint = `/search/code?q=${encodeURIComponent(query)}`;
  if (page) endpoint += `&page=${page}`;
  if (perPage) endpoint += `&per_page=${perPage}`;
  
  return await githubApiRequest(endpoint);
}

export async function searchRepositories(query, page, perPage) {
  let endpoint = `/search/repositories?q=${encodeURIComponent(query)}`;
  if (page) endpoint += `&page=${page}`;
  if (perPage) endpoint += `&per_page=${perPage}`;
  
  return await githubApiRequest(endpoint);
}

export async function pushFiles(owner, repo, branch, files, message) {
  // Parse files JSON
  const filesData = JSON.parse(files);
  
  // This is a simplified implementation
  // In reality, you'd need to create a tree and commit
  throw new Error("pushFiles is not yet implemented - use createOrUpdateFile for single file operations");
}

// Issue operations
export async function listIssues(owner, repo, state, labels, page, perPage) {
  let endpoint = `/repos/${owner}/${repo}/issues`;
  const params = new URLSearchParams();
  if (state) params.append("state", state);
  if (labels) params.append("labels", labels);
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function issueRead(owner, repo, issueNumber, method) {
  let endpoint;
  switch (method) {
    case "get":
      endpoint = `/repos/${owner}/${repo}/issues/${issueNumber}`;
      break;
    case "get_comments":
      endpoint = `/repos/${owner}/${repo}/issues/${issueNumber}/comments`;
      break;
    case "get_labels":
      endpoint = `/repos/${owner}/${repo}/issues/${issueNumber}/labels`;
      break;
    default:
      throw new Error(`Unknown method: ${method}`);
  }
  
  return await githubApiRequest(endpoint);
}

export async function issueWrite(owner, repo, method, params) {
  const paramsData = JSON.parse(params);
  
  let endpoint;
  let fetchOptions = {};
  
  switch (method) {
    case "create":
      endpoint = `/repos/${owner}/${repo}/issues`;
      fetchOptions = {
        method: "POST",
        body: JSON.stringify(paramsData)
      };
      break;
    case "update":
      endpoint = `/repos/${owner}/${repo}/issues/${paramsData.issue_number}`;
      fetchOptions = {
        method: "PATCH",
        body: JSON.stringify(paramsData)
      };
      break;
    case "close":
      endpoint = `/repos/${owner}/${repo}/issues/${paramsData.issue_number}`;
      fetchOptions = {
        method: "PATCH",
        body: JSON.stringify({ state: "closed" })
      };
      break;
    default:
      throw new Error(`Unknown method: ${method}`);
  }
  
  return await githubApiRequest(endpoint, fetchOptions);
}

export async function addIssueComment(owner, repo, issueNumber, body) {
  return await githubApiRequest(`/repos/${owner}/${repo}/issues/${issueNumber}/comments`, {
    method: "POST",
    body: JSON.stringify({ body })
  });
}

export async function searchIssues(query, page, perPage) {
  let endpoint = `/search/issues?q=${encodeURIComponent(query)}`;
  if (page) endpoint += `&page=${page}`;
  if (perPage) endpoint += `&per_page=${perPage}`;
  
  return await githubApiRequest(endpoint);
}

export async function listIssueTypes(owner) {
  // This is a GraphQL-specific feature, not available in REST API
  throw new Error("listIssueTypes requires GraphQL API - not implemented in REST API version");
}

export async function subIssueWrite(owner, repo, issueNumber, action, subIssueNumber) {
  // This is a GraphQL-specific feature for sub-issues
  throw new Error("subIssueWrite requires GraphQL API - not implemented in REST API version");
}

export async function assignCopilotToIssue(owner, repo, issueNumber, task) {
  // This is a specialized GitHub Copilot feature
  throw new Error("assignCopilotToIssue is a GitHub Copilot-specific feature");
}

// Pull Request operations
export async function listPullRequests(owner, repo, state, head, base, page, perPage) {
  let endpoint = `/repos/${owner}/${repo}/pulls`;
  const params = new URLSearchParams();
  if (state) params.append("state", state);
  if (head) params.append("head", head);
  if (base) params.append("base", base);
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function createPullRequest(owner, repo, title, head, base, body, draft) {
  const requestBody = {
    title,
    head,
    base,
    draft
  };
  if (body) {
    requestBody.body = body;
  }
  
  return await githubApiRequest(`/repos/${owner}/${repo}/pulls`, {
    method: "POST",
    body: JSON.stringify(requestBody)
  });
}

export async function pullRequestRead(owner, repo, pullNumber, method) {
  let endpoint;
  switch (method) {
    case "get":
      endpoint = `/repos/${owner}/${repo}/pulls/${pullNumber}`;
      break;
    case "get_diff":
      return await githubApiRequest(`/repos/${owner}/${repo}/pulls/${pullNumber}`, {
        headers: {
          "Accept": "application/vnd.github.diff"
        }
      });
    case "get_files":
      endpoint = `/repos/${owner}/${repo}/pulls/${pullNumber}/files`;
      break;
    case "get_comments":
      endpoint = `/repos/${owner}/${repo}/pulls/${pullNumber}/comments`;
      break;
    case "get_reviews":
      endpoint = `/repos/${owner}/${repo}/pulls/${pullNumber}/reviews`;
      break;
    case "get_status":
      // Get commits first, then status of the head
      const prData = JSON.parse(await githubApiRequest(`/repos/${owner}/${repo}/pulls/${pullNumber}`));
      endpoint = `/repos/${owner}/${repo}/commits/${prData.head.sha}/status`;
      break;
    default:
      throw new Error(`Unknown method: ${method}`);
  }
  
  return await githubApiRequest(endpoint);
}

export async function updatePullRequest(owner, repo, pullNumber, params) {
  const paramsData = JSON.parse(params);
  
  return await githubApiRequest(`/repos/${owner}/${repo}/pulls/${pullNumber}`, {
    method: "PATCH",
    body: JSON.stringify(paramsData)
  });
}

export async function mergePullRequest(owner, repo, pullNumber, mergeMethod) {
  const body = {};
  if (mergeMethod) {
    body.merge_method = mergeMethod;
  }
  
  return await githubApiRequest(`/repos/${owner}/${repo}/pulls/${pullNumber}/merge`, {
    method: "PUT",
    body: JSON.stringify(body)
  });
}

export async function searchPullRequests(query, page, perPage) {
  // Pull requests are issues in the search API
  let endpoint = `/search/issues?q=${encodeURIComponent(query)}+type:pr`;
  if (page) endpoint += `&page=${page}`;
  if (perPage) endpoint += `&per_page=${perPage}`;
  
  return await githubApiRequest(endpoint);
}

export async function pullRequestReviewWrite(owner, repo, pullNumber, action, params) {
  const paramsData = JSON.parse(params);
  
  let endpoint;
  let fetchOptions = {};
  
  switch (action) {
    case "create":
      endpoint = `/repos/${owner}/${repo}/pulls/${pullNumber}/reviews`;
      fetchOptions = {
        method: "POST",
        body: JSON.stringify(paramsData)
      };
      break;
    case "submit":
      endpoint = `/repos/${owner}/${repo}/pulls/${pullNumber}/reviews/${paramsData.review_id}/events`;
      fetchOptions = {
        method: "POST",
        body: JSON.stringify({ event: paramsData.event })
      };
      break;
    case "delete":
      endpoint = `/repos/${owner}/${repo}/pulls/${pullNumber}/reviews/${paramsData.review_id}`;
      fetchOptions = {
        method: "DELETE"
      };
      break;
    default:
      throw new Error(`Unknown action: ${action}`);
  }
  
  return await githubApiRequest(endpoint, fetchOptions);
}

export async function addCommentToPendingReview(owner, repo, pullNumber, path, body, line) {
  const requestBody = {
    path,
    body,
    position: line || 1
  };
  
  return await githubApiRequest(`/repos/${owner}/${repo}/pulls/${pullNumber}/comments`, {
    method: "POST",
    body: JSON.stringify(requestBody)
  });
}

export async function requestCopilotReview(owner, repo, pullNumber) {
  // This is a GitHub Copilot-specific feature
  throw new Error("requestCopilotReview is a GitHub Copilot-specific feature");
}

export async function updatePullRequestBranch(owner, repo, pullNumber) {
  return await githubApiRequest(`/repos/${owner}/${repo}/pulls/${pullNumber}/update-branch`, {
    method: "PUT"
  });
}

// GitHub Actions / Workflows
export async function listWorkflows(owner, repo, page, perPage) {
  let endpoint = `/repos/${owner}/${repo}/actions/workflows`;
  const params = new URLSearchParams();
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function listWorkflowRuns(owner, repo, workflowId, page, perPage) {
  let endpoint = `/repos/${owner}/${repo}/actions/workflows/${workflowId}/runs`;
  const params = new URLSearchParams();
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function getWorkflowRun(owner, repo, runId) {
  return await githubApiRequest(`/repos/${owner}/${repo}/actions/runs/${runId}`);
}

export async function getWorkflowRunUsage(owner, repo, runId) {
  return await githubApiRequest(`/repos/${owner}/${repo}/actions/runs/${runId}/timing`);
}

export async function cancelWorkflowRun(owner, repo, runId) {
  return await githubApiRequest(`/repos/${owner}/${repo}/actions/runs/${runId}/cancel`, {
    method: "POST"
  });
}

export async function rerunWorkflowRun(owner, repo, runId) {
  return await githubApiRequest(`/repos/${owner}/${repo}/actions/runs/${runId}/rerun`, {
    method: "POST"
  });
}

export async function rerunFailedJobs(owner, repo, runId) {
  return await githubApiRequest(`/repos/${owner}/${repo}/actions/runs/${runId}/rerun-failed-jobs`, {
    method: "POST"
  });
}

export async function runWorkflow(owner, repo, workflowId, ref, inputs) {
  const body = { ref };
  if (inputs) {
    body.inputs = JSON.parse(inputs);
  }
  
  return await githubApiRequest(`/repos/${owner}/${repo}/actions/workflows/${workflowId}/dispatches`, {
    method: "POST",
    body: JSON.stringify(body)
  });
}

export async function listWorkflowJobs(owner, repo, runId, page, perPage) {
  let endpoint = `/repos/${owner}/${repo}/actions/runs/${runId}/jobs`;
  const params = new URLSearchParams();
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function getJobLogs(owner, repo, jobId, runId) {
  return await githubApiRequest(`/repos/${owner}/${repo}/actions/jobs/${jobId}/logs`);
}

export async function listWorkflowRunArtifacts(owner, repo, runId, page, perPage) {
  let endpoint = `/repos/${owner}/${repo}/actions/runs/${runId}/artifacts`;
  const params = new URLSearchParams();
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function downloadWorkflowRunArtifact(owner, repo, artifactId) {
  return await githubApiRequest(`/repos/${owner}/${repo}/actions/artifacts/${artifactId}/zip`);
}

export async function getWorkflowRunLogs(owner, repo, runId) {
  return await githubApiRequest(`/repos/${owner}/${repo}/actions/runs/${runId}/logs`);
}

export async function deleteWorkflowRunLogs(owner, repo, runId) {
  return await githubApiRequest(`/repos/${owner}/${repo}/actions/runs/${runId}/logs`, {
    method: "DELETE"
  });
}

// Labels
export async function listLabel(owner, repo, page, perPage) {
  let endpoint = `/repos/${owner}/${repo}/labels`;
  const params = new URLSearchParams();
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function getLabel(owner, repo, name) {
  return await githubApiRequest(`/repos/${owner}/${repo}/labels/${encodeURIComponent(name)}`);
}

export async function labelWrite(owner, repo, action, params) {
  const paramsData = JSON.parse(params);
  
  let endpoint;
  let fetchOptions = {};
  
  switch (action) {
    case "create":
      endpoint = `/repos/${owner}/${repo}/labels`;
      fetchOptions = {
        method: "POST",
        body: JSON.stringify(paramsData)
      };
      break;
    case "update":
      endpoint = `/repos/${owner}/${repo}/labels/${encodeURIComponent(paramsData.current_name)}`;
      fetchOptions = {
        method: "PATCH",
        body: JSON.stringify(paramsData)
      };
      break;
    case "delete":
      endpoint = `/repos/${owner}/${repo}/labels/${encodeURIComponent(paramsData.name)}`;
      fetchOptions = {
        method: "DELETE"
      };
      break;
    default:
      throw new Error(`Unknown action: ${action}`);
  }
  
  return await githubApiRequest(endpoint, fetchOptions);
}

// User operations
export async function getMe() {
  return await githubApiRequest("/user");
}

export async function searchUsers(query, page, perPage) {
  let endpoint = `/search/users?q=${encodeURIComponent(query)}`;
  if (page) endpoint += `&page=${page}`;
  if (perPage) endpoint += `&per_page=${perPage}`;
  
  return await githubApiRequest(endpoint);
}

// Organization operations
export async function searchOrgs(query, page, perPage) {
  let endpoint = `/search/users?q=${encodeURIComponent(query)}+type:org`;
  if (page) endpoint += `&page=${page}`;
  if (perPage) endpoint += `&per_page=${perPage}`;
  
  return await githubApiRequest(endpoint);
}

export async function getTeams(org, page, perPage) {
  let endpoint = `/orgs/${org}/teams`;
  const params = new URLSearchParams();
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function getTeamMembers(org, teamSlug, page, perPage) {
  let endpoint = `/orgs/${org}/teams/${teamSlug}/members`;
  const params = new URLSearchParams();
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

// Gist operations
export async function listGists(page, perPage) {
  let endpoint = "/gists";
  const params = new URLSearchParams();
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function getGist(gistId) {
  return await githubApiRequest(`/gists/${gistId}`);
}

export async function createGist(description, files, isPublic) {
  const filesData = JSON.parse(files);
  const body = {
    files: filesData,
    public: isPublic
  };
  if (description) {
    body.description = description;
  }
  
  return await githubApiRequest("/gists", {
    method: "POST",
    body: JSON.stringify(body)
  });
}

export async function updateGist(gistId, description, files) {
  const body = {};
  if (description) {
    body.description = description;
  }
  if (files) {
    body.files = JSON.parse(files);
  }
  
  return await githubApiRequest(`/gists/${gistId}`, {
    method: "PATCH",
    body: JSON.stringify(body)
  });
}

// Discussions
export async function listDiscussions(owner, repo, category, page, perPage) {
  // Discussions require GraphQL API
  throw new Error("Discussions require GraphQL API - not implemented in REST API version");
}

export async function getDiscussion(owner, repo, discussionNumber) {
  throw new Error("Discussions require GraphQL API - not implemented in REST API version");
}

export async function getDiscussionComments(owner, repo, discussionNumber, page, perPage) {
  throw new Error("Discussions require GraphQL API - not implemented in REST API version");
}

export async function listDiscussionCategories(owner, repo) {
  throw new Error("Discussions require GraphQL API - not implemented in REST API version");
}

// Notifications
export async function listNotifications(all, page, perPage) {
  let endpoint = "/notifications";
  const params = new URLSearchParams();
  if (all) params.append("all", "true");
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function getNotificationDetails(threadId) {
  return await githubApiRequest(`/notifications/threads/${threadId}`);
}

export async function markAllNotificationsRead() {
  return await githubApiRequest("/notifications", {
    method: "PUT"
  });
}

export async function dismissNotification(threadId) {
  return await githubApiRequest(`/notifications/threads/${threadId}`, {
    method: "DELETE"
  });
}

export async function manageNotificationSubscription(threadId, subscribed) {
  return await githubApiRequest(`/notifications/threads/${threadId}/subscription`, {
    method: "PUT",
    body: JSON.stringify({ subscribed })
  });
}

export async function manageRepositoryNotificationSubscription(owner, repo, subscribed) {
  return await githubApiRequest(`/repos/${owner}/${repo}/subscription`, {
    method: "PUT",
    body: JSON.stringify({ subscribed })
  });
}

// Security - Code Scanning
export async function listCodeScanningAlerts(owner, repo, state, ref, page, perPage) {
  let endpoint = `/repos/${owner}/${repo}/code-scanning/alerts`;
  const params = new URLSearchParams();
  if (state) params.append("state", state);
  if (ref) params.append("ref", ref);
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function getCodeScanningAlert(owner, repo, alertNumber) {
  return await githubApiRequest(`/repos/${owner}/${repo}/code-scanning/alerts/${alertNumber}`);
}

// Security - Secret Scanning
export async function listSecretScanningAlerts(owner, repo, state, page, perPage) {
  let endpoint = `/repos/${owner}/${repo}/secret-scanning/alerts`;
  const params = new URLSearchParams();
  if (state) params.append("state", state);
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function getSecretScanningAlert(owner, repo, alertNumber) {
  return await githubApiRequest(`/repos/${owner}/${repo}/secret-scanning/alerts/${alertNumber}`);
}

// Security - Dependabot
export async function listDependabotAlerts(owner, repo, state, page, perPage) {
  let endpoint = `/repos/${owner}/${repo}/dependabot/alerts`;
  const params = new URLSearchParams();
  if (state) params.append("state", state);
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function getDependabotAlert(owner, repo, alertNumber) {
  return await githubApiRequest(`/repos/${owner}/${repo}/dependabot/alerts/${alertNumber}`);
}

// Security - Security Advisories
export async function listGlobalSecurityAdvisories(cveId, ghsaId, page, perPage) {
  let endpoint = "/advisories";
  const params = new URLSearchParams();
  if (cveId) params.append("cve_id", cveId);
  if (ghsaId) params.append("ghsa_id", ghsaId);
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function getGlobalSecurityAdvisory(advisoryId) {
  return await githubApiRequest(`/advisories/${advisoryId}`);
}

export async function listRepositorySecurityAdvisories(owner, repo, state, page, perPage) {
  let endpoint = `/repos/${owner}/${repo}/security-advisories`;
  const params = new URLSearchParams();
  if (state) params.append("state", state);
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function listOrgRepositorySecurityAdvisories(org, state, page, perPage) {
  let endpoint = `/orgs/${org}/security-advisories`;
  const params = new URLSearchParams();
  if (state) params.append("state", state);
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

// Stars
export async function listStarredRepositories(page, perPage) {
  let endpoint = "/user/starred";
  const params = new URLSearchParams();
  if (page) params.append("page", page.toString());
  if (perPage) params.append("per_page", perPage.toString());
  if (params.toString()) endpoint += `?${params}`;
  
  return await githubApiRequest(endpoint);
}

export async function starRepository(owner, repo) {
  return await githubApiRequest(`/user/starred/${owner}/${repo}`, {
    method: "PUT"
  });
}

export async function unstarRepository(owner, repo) {
  return await githubApiRequest(`/user/starred/${owner}/${repo}`, {
    method: "DELETE"
  });
}

// Projects
export async function listProjects(owner, ownerType, page, perPage) {
  // Projects V2 require GraphQL API
  throw new Error("Projects V2 require GraphQL API - not implemented in REST API version");
}

export async function getProject(owner, ownerType, projectNumber) {
  throw new Error("Projects V2 require GraphQL API - not implemented in REST API version");
}

export async function listProjectFields(owner, ownerType, projectNumber, page, perPage) {
  throw new Error("Projects V2 require GraphQL API - not implemented in REST API version");
}

export async function getProjectField(owner, ownerType, projectNumber, fieldId) {
  throw new Error("Projects V2 require GraphQL API - not implemented in REST API version");
}

export async function listProjectItems(owner, ownerType, projectNumber, query, page, perPage) {
  throw new Error("Projects V2 require GraphQL API - not implemented in REST API version");
}

export async function getProjectItem(owner, ownerType, projectNumber, itemId) {
  throw new Error("Projects V2 require GraphQL API - not implemented in REST API version");
}

export async function addProjectItem(owner, ownerType, projectNumber, contentId) {
  throw new Error("Projects V2 require GraphQL API - not implemented in REST API version");
}

export async function updateProjectItem(owner, ownerType, projectNumber, itemId, fieldId, value) {
  throw new Error("Projects V2 require GraphQL API - not implemented in REST API version");
}

export async function deleteProjectItem(owner, ownerType, projectNumber, itemId) {
  throw new Error("Projects V2 require GraphQL API - not implemented in REST API version");
}

// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

package main

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"

	"gomodule-server-go/gen/local/gomodule-server/gomodule"

	wasihttp "github.com/ydnar/wasi-http-go/wasihttp"
	"go.bytecodealliance.org/cm"
)

func init() {
	gomodule.Exports.GetLatestVersions = getLatestVersions
	gomodule.Exports.GetModuleInfo = getModuleInfo
}

type GetLatestVersionsResult = cm.Result[string, string, string]
type GetModuleInfoResult = cm.Result[string, string, string]

func httpRequest(url string) ([]byte, error) {
	client := &http.Client{
		Transport: &wasihttp.Transport{},
	}

	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %v", err)
	}

	req.Header.Set("User-Agent", "hyper-mcp/1.0")

	resp, err := client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("HTTP request failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("HTTP request failed with status: %d", resp.StatusCode)
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("failed to read response body: %v", err)
	}

	return body, nil
}

func getLatestVersions(moduleNames string) GetLatestVersionsResult {
	modules := strings.Split(moduleNames, ",")
	results := make(map[string]string)

	for _, moduleName := range modules {
		moduleName = strings.TrimSpace(moduleName)
		if moduleName == "" {
			continue
		}

		if !strings.Contains(moduleName, "/") {
			moduleName = "github.com/" + moduleName
		}

		url := fmt.Sprintf("https://proxy.golang.org/%s/@latest", moduleName)

		data, err := httpRequest(url)
		if err != nil {
			return cm.Err[GetLatestVersionsResult](fmt.Sprintf("Failed to fetch %s: %v", moduleName, err))
		}

		var moduleInfo map[string]interface{}
		if err := json.Unmarshal(data, &moduleInfo); err != nil {
			return cm.Err[GetLatestVersionsResult](fmt.Sprintf("Failed to parse JSON for %s: %v", moduleName, err))
		}

		if version, ok := moduleInfo["Version"].(string); ok {
			results[moduleName] = version
		}
	}

	if len(results) == 0 {
		return cm.Err[GetLatestVersionsResult]("Failed to get latest versions")
	}

	jsonData, err := json.Marshal(results)
	if err != nil {
		return cm.Err[GetLatestVersionsResult](fmt.Sprintf("Failed to marshal results: %v", err))
	}

	return cm.OK[GetLatestVersionsResult](string(jsonData))
}

func getModuleInfo(moduleNames string) GetModuleInfoResult {
	modules := strings.Split(moduleNames, ",")
	var results []map[string]interface{}

	for _, moduleName := range modules {
		moduleName = strings.TrimSpace(moduleName)
		if moduleName == "" {
			continue
		}

		if !strings.Contains(moduleName, "/") {
			moduleName = "github.com/" + moduleName
		}

		url := fmt.Sprintf("https://proxy.golang.org/%s/@latest", moduleName)

		data, err := httpRequest(url)
		if err != nil {
			return cm.Err[GetModuleInfoResult](fmt.Sprintf("Failed to fetch %s: %v", moduleName, err))
		}

		var moduleInfo map[string]interface{}
		if err := json.Unmarshal(data, &moduleInfo); err != nil {
			return cm.Err[GetModuleInfoResult](fmt.Sprintf("Failed to parse JSON for %s: %v", moduleName, err))
		}

		results = append(results, moduleInfo)
	}

	if len(results) == 0 {
		return cm.Err[GetModuleInfoResult]("Failed to get module information")
	}

	jsonData, err := json.Marshal(results)
	if err != nil {
		return cm.Err[GetModuleInfoResult](fmt.Sprintf("Failed to marshal results: %v", err))
	}

	return cm.OK[GetModuleInfoResult](string(jsonData))
}

func main() {}

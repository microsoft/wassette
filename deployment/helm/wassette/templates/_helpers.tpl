{{/*
Expand the name of the chart.
*/}}
{{- define "wassette.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
*/}}
{{- define "wassette.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "wassette.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "wassette.labels" -}}
helm.sh/chart: {{ include "wassette.chart" . }}
{{ include "wassette.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "wassette.selectorLabels" -}}
app.kubernetes.io/name: {{ include "wassette.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Create the name of the service account to use
*/}}
{{- define "wassette.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "wassette.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
Get the image tag
*/}}
{{- define "wassette.imageTag" -}}
{{- .Values.image.tag | default .Chart.AppVersion }}
{{- end }}

{{/*
Get the command based on transport
*/}}
{{- define "wassette.command" -}}
{{- if eq .Values.wassette.transport "streamable-http" -}}
- wassette
- serve
- --streamable-http
{{- else if eq .Values.wassette.transport "sse" -}}
- wassette
- serve
- --sse
{{- else if eq .Values.wassette.transport "stdio" -}}
- wassette
- serve
- --stdio
{{- else -}}
- wassette
- serve
- --streamable-http
{{- end -}}
{{- end }}

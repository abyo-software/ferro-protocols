{{/*
SPDX-License-Identifier: Apache-2.0
*/}}

{{- define "ferro-oci-server.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "ferro-oci-server.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- $name := default .Chart.Name .Values.nameOverride -}}
{{- if contains $name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{- define "ferro-oci-server.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "ferro-oci-server.labels" -}}
helm.sh/chart: {{ include "ferro-oci-server.chart" . }}
{{ include "ferro-oci-server.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: ferro-protocols
{{- end -}}

{{- define "ferro-oci-server.selectorLabels" -}}
app.kubernetes.io/name: {{ include "ferro-oci-server.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "ferro-oci-server.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (include "ferro-oci-server.fullname" .) .Values.serviceAccount.name -}}
{{- else -}}
{{- default "default" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}

{{/*
PVC claim name — an existing claim takes precedence over the templated one.
*/}}
{{- define "ferro-oci-server.pvcName" -}}
{{- if .Values.persistence.existingClaim -}}
{{- .Values.persistence.existingClaim -}}
{{- else -}}
{{- printf "%s-data" (include "ferro-oci-server.fullname" .) -}}
{{- end -}}
{{- end -}}

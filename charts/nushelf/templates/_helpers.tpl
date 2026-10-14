{{- define "nushelf.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "nushelf.fullname" -}}
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

{{- define "nushelf.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "nushelf.labels" -}}
helm.sh/chart: {{ include "nushelf.chart" . }}
{{ include "nushelf.selectorLabels" . }}
app.kubernetes.io/version: {{ .Values.image.tag | default .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "nushelf.selectorLabels" -}}
app.kubernetes.io/name: {{ include "nushelf.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "nushelf.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "nushelf.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{- define "nushelf.secretName" -}}
{{- default (include "nushelf.fullname" .) .Values.secrets.existingSecret }}
{{- end }}

{{- define "nushelf.image" -}}
{{- printf "%s:%s" .Values.image.repository (.Values.image.tag | default .Chart.AppVersion) }}
{{- end }}

{{/* True when packages are kept on a ReadWriteOnce volume, which pins the release to one pod. */}}
{{- define "nushelf.singleWriterVolume" -}}
{{- if and (eq .Values.config.storageProvider "local") .Values.persistence.enabled (has "ReadWriteOnce" .Values.persistence.accessModes) }}true{{- end }}
{{- end }}

{{- define "nushelf.validate" -}}
{{- $url := required "publicUrl is required, e.g. https://nuget.example.com" .Values.publicUrl }}
{{- if not (regexMatch "^https?://[^/?#]+/?$" $url) }}
{{- fail "publicUrl must be an http(s) origin without a path, query or credentials" }}
{{- end }}
{{- if not (has .Values.config.storageProvider (list "local" "s3" "azure" "gcs")) }}
{{- fail "config.storageProvider must be one of local, s3, azure or gcs" }}
{{- end }}
{{- if and (eq .Values.config.storageProvider "s3") (not .Values.storage.s3.bucket) }}
{{- fail "storage.s3.bucket is required when config.storageProvider is s3" }}
{{- end }}
{{- if and (include "nushelf.singleWriterVolume" .) (gt (int .Values.replicaCount) 1) }}
{{- fail "replicaCount > 1 needs shared storage; a ReadWriteOnce local volume supports one replica" }}
{{- end }}
{{- if and (eq .Values.config.storageProvider "local") (not .Values.persistence.enabled) (gt (int .Values.replicaCount) 1) }}
{{- fail "replicaCount > 1 with local storage needs persistence on a ReadWriteMany volume" }}
{{- end }}
{{- if and .Values.metrics.enabled (eq (int .Values.metrics.port) 5000) }}
{{- fail "metrics.port must differ from the registry port 5000" }}
{{- end }}
{{- if and .Values.httpRoute.enabled (not .Values.httpRoute.parentRefs) }}
{{- fail "httpRoute.parentRefs needs at least one Gateway when httpRoute is enabled" }}
{{- end }}
{{- if not .Values.secrets.existingSecret }}
{{- if lt (len .Values.secrets.adminApiKey) 32 }}
{{- fail "Set secrets.existingSecret, or secrets.adminApiKey with at least 32 characters" }}
{{- end }}
{{- if not .Values.secrets.databaseUrl }}
{{- fail "Set secrets.existingSecret, or secrets.databaseUrl" }}
{{- end }}
{{- end }}
{{- end }}

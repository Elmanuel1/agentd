{{- define "agentd.labels" -}}
app.kubernetes.io/name: agentd
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{- end -}}

{{- define "agentd.securityContext" -}}
allowPrivilegeEscalation: false
readOnlyRootFilesystem: true
runAsNonRoot: true
capabilities:
  drop: [ALL]
seccompProfile:
  type: RuntimeDefault
{{- end -}}

{{- define "agentd.databaseUrl" -}}
- name: DATABASE_URL
  valueFrom:
    secretKeyRef:
      name: {{ .Values.database.existingSecret }}
      key: {{ .Values.database.urlKey }}
{{- end -}}

{{- /* Where a pod runs: each setting from the role (or the migrations) when it sets one, else the chart-wide
value. */ -}}
{{- define "agentd.scheduling" -}}
{{- range $key := list "nodeSelector" "tolerations" "affinity" "topologySpreadConstraints" "priorityClassName" }}
{{- with (index $.spec $key) | default (index $.Values $key) }}
{{ $key }}: {{- if kindIs "string" . }} {{ . }}{{ else }}{{ toYaml . | nindent 2 }}{{ end }}
{{- end }}
{{- end }}
{{- end -}}


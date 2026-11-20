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

{{- define "agentd.databaseCredentials" -}}
- name: PGUSER
  valueFrom:
    secretKeyRef:
      name: {{ .Values.database.existingSecret }}
      key: {{ .Values.database.userKey }}
- name: PGPASSWORD
  valueFrom:
    secretKeyRef:
      name: {{ .Values.database.existingSecret }}
      key: {{ .Values.database.passwordKey }}
{{- end -}}

{{- /* agentd's DATABASE_URL in libpq's key=value form, so the password needs no URL encoding. */ -}}
{{- define "agentd.databaseUrl" -}}
{{- include "agentd.databaseCredentials" . }}
- name: DATABASE_URL
  value: "host={{ .Values.database.host }} port={{ .Values.database.port }} dbname={{ .Values.database.name }} user=$(PGUSER) password=$(PGPASSWORD) sslmode={{ .Values.database.sslMode }}"
{{- end -}}

{{- /* Where a pod runs: each setting from the role (or the migrations) when it has the key, even set empty to
clear it, else the chart-wide value. */ -}}
{{- define "agentd.scheduling" -}}
{{- range $key := list "nodeSelector" "tolerations" "affinity" "topologySpreadConstraints" "priorityClassName" }}
{{- $value := ternary (index $.spec $key) (index $.Values $key) (hasKey $.spec $key) }}
{{- with $value }}
{{ $key }}: {{- if kindIs "string" . }} {{ . }}{{ else }}{{ toYaml . | nindent 2 }}{{ end }}
{{- end }}
{{- end }}
{{- end -}}


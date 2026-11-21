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
      name: {{ .Values.database.secretName }}
      key: {{ .Values.database.userKey }}
- name: PGPASSWORD
  valueFrom:
    secretKeyRef:
      name: {{ .Values.database.secretName }}
      key: {{ .Values.database.passwordKey }}
{{- end -}}

{{- /* An image by digest when one is set, else by tag. */ -}}
{{- define "agentd.image" -}}
{{- if .digest }}{{ .repository }}@{{ .digest }}{{ else }}{{ .repository }}:{{ .tag }}{{ end }}
{{- end -}}

{{- define "agentd.databaseCaPath" -}}
/etc/agentd/database-ca/{{ .Values.database.caBundle.key }}
{{- end -}}

{{- /* agentd's DATABASE_URL in libpq's key=value form, so the password needs no URL encoding. The password is
single-quoted: a space in it stays in it instead of starting another keyword. */ -}}
{{- define "agentd.databaseUrl" -}}
{{- include "agentd.databaseCredentials" . }}
- name: DATABASE_URL
  value: "host={{ .Values.database.host }} port={{ .Values.database.port }} dbname={{ .Values.database.name }} user=$(PGUSER) password='$(PGPASSWORD)' sslmode={{ .Values.database.sslMode }}{{ if .Values.database.caBundle.configMapName }} sslrootcert={{ include "agentd.databaseCaPath" . }}{{ end }}"
{{- end -}}

{{- /* Flyway's JDBC URL with the same TLS: the CA bundle when one is mounted, else Java's own trust store for the
verify modes, as agentd uses the operating system's roots. */ -}}
{{- define "agentd.flywayUrl" -}}
jdbc:postgresql://{{ .Values.database.host }}:{{ .Values.database.port }}/{{ .Values.database.name }}?sslmode={{ .Values.database.sslMode }}
{{- if .Values.database.caBundle.configMapName }}&sslrootcert={{ include "agentd.databaseCaPath" . }}
{{- else if hasPrefix "verify-" .Values.database.sslMode }}&sslfactory=org.postgresql.ssl.DefaultJavaSSLFactory
{{- end }}
{{- end -}}

{{- /* The CA bundle's volume and mount, for a pod spec and its container. */ -}}
{{- define "agentd.databaseCaVolume" -}}
{{- with .Values.database.caBundle.configMapName }}
- name: database-ca
  configMap:
    name: {{ . }}
{{- end }}
{{- end -}}

{{- define "agentd.databaseCaMount" -}}
{{- if .Values.database.caBundle.configMapName }}
- name: database-ca
  mountPath: /etc/agentd/database-ca
  readOnly: true
{{- end }}
{{- end -}}

{{- /* The pod-level security settings every agentd pod and the migrations share. */ -}}
{{- define "agentd.podSecurityContext" -}}
runAsUser: 10001
runAsGroup: 10001
runAsNonRoot: true
fsGroup: 10001
seccompProfile:
  type: RuntimeDefault
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


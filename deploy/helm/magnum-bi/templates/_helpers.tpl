{{- define "magnum.name" -}}{{ .Release.Name }}-magnum-bi{{- end -}}
{{- define "magnum.scramdb" -}}{{ .Release.Name }}-scramdb{{- end -}}
{{- define "magnum.labels" -}}
app.kubernetes.io/part-of: magnum-bi
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{- end -}}
{{- define "magnum.selector" -}}
app.kubernetes.io/name: magnum-bi
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}
{{- define "magnum.scramdbSelector" -}}
app.kubernetes.io/name: scramdb
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}
{{- define "magnum.host" -}}
{{- if .Values.ingress.host }}{{ .Values.ingress.host }}{{ else }}{{ regexReplaceAll "^https?://([^/:]+).*$" .Values.publicUrl "${1}" }}{{ end -}}
{{- end -}}

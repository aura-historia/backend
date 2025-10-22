FROM python:3.11-slim

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY . .

ENV DEVICE=${DEVICE:-cpu}
EXPOSE 8000

CMD ["uvicorn", "embed_server:app", "--host", "0.0.0.0", "--port", "8000"]

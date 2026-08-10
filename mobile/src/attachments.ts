import type { ChatAttachment } from "./types";
import { panesConnectionManager } from "./stores/panes-connection";

declare const plus: any;

interface LocalFileData {
  dataUrl: string;
  fileName: string;
  sizeBytes: number;
  mimeType: string;
}

function fileNameFromPath(path: string, fallbackName: string) {
  const value = path.split(/[\\/]/).pop() || fallbackName;
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

function readWithPlus(filePath: string, fallbackName: string, fallbackMimeType: string): Promise<LocalFileData> {
  return new Promise((resolve, reject) => {
    plus.io.resolveLocalFileSystemURL(filePath, (entry: any) => {
      entry.file((file: any) => {
        if (Number(file.size) > 10 * 1024 * 1024) {
          reject(new Error("单个附件不能超过 10 MB"));
          return;
        }
        const reader = new plus.io.FileReader();
        reader.onloadend = () => {
          const dataUrl = String(reader.result || "");
          if (!dataUrl.startsWith("data:") || !dataUrl.includes(";base64,")) {
            reject(new Error("附件数据格式无效"));
            return;
          }
          resolve({
            dataUrl,
            fileName: String(file.name || entry.name || fallbackName),
            sizeBytes: Number(file.size) || 0,
            mimeType: String(file.type || fallbackMimeType),
          });
        };
        reader.onerror = () => reject(new Error("读取附件失败"));
        reader.readAsDataURL(file);
      }, () => reject(new Error("读取附件失败")));
    }, () => reject(new Error("无法访问所选附件")));
  });
}

async function readWithFetch(filePath: string, fallbackName: string, fallbackMimeType: string): Promise<LocalFileData> {
  const response = await fetch(filePath);
  if (!response.ok) throw new Error("读取附件失败");
  const blob = await response.blob();
  if (blob.size > 10 * 1024 * 1024) throw new Error("单个附件不能超过 10 MB");
  const dataUrl = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result || ""));
    reader.onerror = () => reject(new Error("读取附件失败"));
    reader.readAsDataURL(blob);
  });
  return {
    dataUrl,
    fileName: fileNameFromPath(filePath, fallbackName),
    sizeBytes: blob.size,
    mimeType: blob.type || fallbackMimeType,
  };
}

async function readLocalFile(filePath: string, fallbackName: string, fallbackMimeType: string) {
  if (typeof plus !== "undefined" && plus.io) return readWithPlus(filePath, fallbackName, fallbackMimeType);
  return readWithFetch(filePath, fallbackName, fallbackMimeType);
}

async function uploadBase64(panesId: string, localFile: LocalFileData, cancelled: () => boolean) {
  const prefix = /^data:([^;,]+);base64,/i.exec(localFile.dataUrl);
  const commaIndex = localFile.dataUrl.indexOf(",");
  if (!prefix || commaIndex < 0) throw new Error("附件数据格式无效");
  const dataBase64 = localFile.dataUrl.slice(commaIndex + 1);
  const chunkSize = 256 * 1024;
  const chunkCount = Math.ceil(dataBase64.length / chunkSize);
  if (chunkCount > 64) throw new Error("单个附件不能超过 10 MB");
  const uploadId = `mobile-${Date.now()}-${Math.random().toString(16).slice(2)}`;
  let completed: ChatAttachment | null = null;
  for (let chunkIndex = 0; chunkIndex < chunkCount; chunkIndex += 1) {
    if (cancelled()) throw new Error("上传已取消");
    completed = await panesConnectionManager.request<ChatAttachment>(panesId, "attachment.upload", {
      upload_id: uploadId,
      file_name: localFile.fileName,
      mime_type: prefix[1].toLowerCase(),
      chunk_index: chunkIndex,
      chunk_count: chunkCount,
      data_base64: dataBase64.slice(chunkIndex * chunkSize, (chunkIndex + 1) * chunkSize),
    });
  }
  if (cancelled()) throw new Error("上传已取消");
  if (!completed?.filePath) throw new Error("附件上传没有返回文件路径");
  return {
    ...completed,
    fileName: localFile.fileName,
    sizeBytes: localFile.sizeBytes || completed.sizeBytes,
    mimeType: localFile.mimeType || completed.mimeType,
    uploading: false,
  };
}

export async function readAndUploadAttachment(
  panesId: string,
  filePath: string,
  fallbackName: string,
  fallbackMimeType: string,
  cancelled: () => boolean,
) {
  const localFile = await readLocalFile(filePath, fallbackName, fallbackMimeType);
  return uploadBase64(panesId, localFile, cancelled);
}

// Feature-scoped translations keep the rich-text renderer independent of the large app dictionary.
export const richTextLabels = (vi: boolean) => vi ? {
  document: 'Tài liệu', email: 'Bản nháp email', recipient: 'Người nhận', copy: 'Sao chép', copied: 'Đã sao chép',
  copyFailed: 'Không sao chép được. Hãy chọn và sao chép nội dung.', code: 'Mã nguồn', source: 'Nguồn',
  fileSource: 'Nguồn tài liệu', missingSource: 'Chỉ có mã tham chiếu; bản ghi chưa chứa đường dẫn nguồn.',
  widget: 'Nội dung tương tác ChatGPT', missingWidget: 'Xem bản gốc trên ChatGPT; bản ghi chưa có dữ liệu để dựng widget.',
  unavailableLink: 'Bản ghi chưa có đường dẫn sử dụng được trong ChatCMD.', unavailableImage: 'Ảnh không có đường dẫn sử dụng được',
  note: 'Ghi chú', tip: 'Mẹo', important: 'Quan trọng', warning: 'Cảnh báo', caution: 'Chú ý',
  images: 'Hình ảnh', links: 'Liên kết tham khảo', files: 'Tài liệu tham khảo', products: 'Sản phẩm', finance: 'Biểu đồ tài chính',
  weather: 'Thời tiết', sports: 'Thể thao', video: 'Video', audio: 'Âm thanh', map: 'Bản đồ', table: 'Bảng dữ liệu', chart: 'Biểu đồ',
} : {
  document: 'Document', email: 'Email draft', recipient: 'Recipient', copy: 'Copy', copied: 'Copied',
  copyFailed: 'Could not copy. Select and copy the content manually.', code: 'Code', source: 'Source',
  fileSource: 'File source', missingSource: 'Only reference IDs are available; this transcript has no source URLs.',
  widget: 'ChatGPT interactive content', missingWidget: 'View the original on ChatGPT; widget data is not included in this transcript.',
  unavailableLink: 'This transcript has no usable ChatCMD link.', unavailableImage: 'Image URL unavailable',
  note: 'Note', tip: 'Tip', important: 'Important', warning: 'Warning', caution: 'Caution',
  images: 'Images', links: 'Reference links', files: 'Reference files', products: 'Products', finance: 'Financial chart',
  weather: 'Weather', sports: 'Sports', video: 'Video', audio: 'Audio', map: 'Map', table: 'Data table', chart: 'Chart',
};
export type RichTextLabels = ReturnType<typeof richTextLabels>;

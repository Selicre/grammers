// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
use std::collections::HashMap;
use std::convert::TryInto;

use grammers_mtsender::InvocationError;
use grammers_tl_types as tl;

use crate::iterators::{RpcIterBuffer, RpcIterator};
use crate::types;
use crate::Client;

const MAX_DIALOGS_PER_REQUEST: i32 = 100;

pub struct Dialogs {
    buffer: RpcIterBuffer<tl::functions::messages::GetDialogs, types::Dialog>,

    // We reuse the same map for the sake of avoiding allocations
    entities: HashMap<i32, types::Entity>,
    messages: HashMap<(i32, i32), tl::enums::Message>,
}

// TODO more reusable methods to get ids from things
fn peer_id(peer: &tl::enums::Peer) -> i32 {
    match peer {
        tl::enums::Peer::User(user) => user.user_id,
        tl::enums::Peer::Chat(chat) => chat.chat_id,
        tl::enums::Peer::Channel(channel) => channel.channel_id,
    }
}

fn message_id(message: &tl::enums::Message) -> Option<(i32, i32)> {
    match message {
        tl::enums::Message::Message(message) => {
            // TODO this will probably fail in pm
            Some((peer_id(&message.to_id), message.id))
        }
        tl::enums::Message::Service(message) => Some((peer_id(&message.to_id), message.id)),
        tl::enums::Message::Empty(_) => None,
    }
}

impl Dialogs {
    pub fn iter() -> Self {
        Self {
            buffer: RpcIterBuffer::new(tl::functions::messages::GetDialogs {
                exclude_pinned: false,
                folder_id: None,
                offset_date: 0,
                offset_id: 0,
                offset_peer: tl::types::InputPeerEmpty {}.into(),
                limit: MAX_DIALOGS_PER_REQUEST,
                hash: 0,
            }),
            entities: HashMap::new(),
            messages: HashMap::new(),
        }
    }

    fn update_user_entities(&mut self, users: Vec<tl::enums::User>) {
        users
            .into_iter()
            .filter_map(|user| {
                if let Ok(user) = user.try_into() {
                    Some(user)
                } else {
                    None
                }
            })
            .for_each(|user: tl::types::User| {
                self.entities.insert(user.id, types::Entity::User(user));
            });
    }

    fn update_chat_entities(&mut self, chats: Vec<tl::enums::Chat>) {
        chats.into_iter().for_each(|chat| match chat {
            tl::enums::Chat::Chat(chat) => {
                self.entities.insert(chat.id, types::Entity::Chat(chat));
            }
            tl::enums::Chat::Channel(channel) => {
                self.entities
                    .insert(channel.id, types::Entity::Channel(channel));
            }
            _ => {}
        });
    }

    fn update_messages(&mut self, messages: Vec<tl::enums::Message>) {
        messages.into_iter().for_each(|message| {
            if let Some(id) = message_id(&message) {
                self.messages.insert(id, message);
            }
        });
    }

    fn update_dialogs(&mut self, dialogs: Vec<tl::enums::Dialog>) {
        dialogs
            .into_iter()
            .rev()
            .for_each(move |dialog| match dialog {
                tl::enums::Dialog::Dialog(dialog) => {
                    let peer_id = peer_id(&dialog.peer);
                    if let Some(entity) = self.entities.remove(&peer_id) {
                        let last_message = self.messages.remove(&(peer_id, dialog.top_message));
                        self.buffer.push(types::Dialog {
                            dialog,
                            entity,
                            last_message,
                        });
                    }
                }
                tl::enums::Dialog::Folder(_) => {}
            });
    }

    fn update_request_offsets(&mut self) {
        if let Some(dialog) = self.buffer.batch.get(0) {
            self.buffer.request.offset_peer = dialog.entity.to_input_peer();
        }

        // Find last dialog with a message
        for dialog in self.buffer.batch.iter() {
            if let Some(message) = &dialog.last_message {
                match message {
                    tl::enums::Message::Message(message) => {
                        self.buffer.request.offset_id = message.id;
                        self.buffer.request.offset_date = message.date;
                    }
                    tl::enums::Message::Service(message) => {
                        self.buffer.request.offset_id = message.id;
                        self.buffer.request.offset_date = message.date;
                    }
                    tl::enums::Message::Empty(message) => {
                        self.buffer.request.offset_id = message.id;
                    }
                }
                break;
            }
        }
    }
}

impl RpcIterator<types::Dialog> for Dialogs {
    fn total(&self) -> Option<usize> {
        self.buffer.total
    }

    fn should_fill_buffer(&self) -> bool {
        self.buffer.should_fill()
    }

    fn pop_buffer(&mut self) -> Option<types::Dialog> {
        self.buffer.pop()
    }

    fn fill_buffer(&mut self, client: &mut Client) -> Result<(), InvocationError> {
        match client.invoke(&self.buffer.request)? {
            tl::enums::messages::Dialogs::Dialogs(tl::types::messages::Dialogs {
                dialogs,
                messages,
                chats,
                users,
            }) => {
                self.buffer.total = Some(dialogs.len());
                self.buffer.done = true;
                self.update_user_entities(users);
                self.update_chat_entities(chats);
                self.update_messages(messages);
                self.update_dialogs(dialogs);
            }
            tl::enums::messages::Dialogs::DialogsSlice(tl::types::messages::DialogsSlice {
                count,
                dialogs,
                messages,
                chats,
                users,
            }) => {
                self.buffer.total = Some(count as usize);
                self.buffer.done = dialogs.len() < self.buffer.request.limit as usize;
                self.update_user_entities(users);
                self.update_chat_entities(chats);
                self.update_messages(messages);
                self.update_dialogs(dialogs);
                self.update_request_offsets();
            }
            tl::enums::messages::Dialogs::DialogsNotModified(dialogs) => {
                self.buffer.total = Some(dialogs.count as usize);
                self.buffer.done = true;
            }
        }
        Ok(())
    }
}
